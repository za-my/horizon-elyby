//! Ely.by accounts (https://ely.by): the Yggdrasil-compatible authserver behind
//! `auth::Provider::ElyBy` (docs/elyby-account.md). A player signs in with the
//! Ely.by login and password once; the game keeps the tokens (`horizon-elyby.json`
//! next to `options.txt`, readable by its owner only), never the password, and
//! joins servers that accept Ely.by the way vanilla joins with a Microsoft
//! session: `session/join` with the access token and the server hash.
//!
//! Unlike the launcher's Microsoft tokens (OS keychain), these live in a file:
//! the game binary has no keychain dependency, and an Ely.by token only grants
//! what the password would for server joins.
use crate::auth::{AuthSession, Provider, parse_uuid};
use std::{fmt, io::Write, path::Path, time::Duration};

pub const DEFAULT_BASE: &str = "https://authserver.ely.by";
/// The kept account, in the folder `horizon-player.txt` lives in.
pub const FILE: &str = "horizon-elyby.json";

/// The authserver: `HORIZON_ELYBY_URL` overrides it (a mock in the harness).
pub fn base_url() -> String {
    normalize_base(std::env::var("HORIZON_ELYBY_URL").ok().as_deref())
}

fn normalize_base(text: Option<&str>) -> String {
    text.map(|t| t.trim().trim_end_matches('/'))
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_BASE)
        .to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The account has two-factor authentication: ask for the code and call
    /// again with `Some(code)`.
    TwoFactorRequired,
    /// Wrong login, password or code.
    InvalidCredentials,
    /// The kept token was refused and can't be renewed: sign in again.
    SessionExpired,
    /// No answer, or a 5xx.
    Unavailable,
    /// Anything else, with the service's message.
    Other(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TwoFactorRequired => {
                f.write_str("This Ely.by account uses two-factor authentication")
            }
            Self::InvalidCredentials => f.write_str("Wrong Ely.by login or password"),
            Self::SessionExpired => f.write_str("The Ely.by session expired: sign in again"),
            Self::Unavailable => f.write_str("Ely.by is not reachable right now"),
            Self::Other(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for Error {}

/// A signed-in Ely.by account with the tokens that keep it signed in.
#[derive(Clone)]
pub struct Account {
    pub name: String,
    pub uuid: u128,
    access_token: String,
    client_token: String,
}
impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("name", &self.name)
            .field("uuid", &horizon_protocol::crypto::uuid_string(self.uuid))
            .field("tokens", &"<hidden>")
            .finish()
    }
}

impl Account {
    /// The session the network code joins servers with.
    pub fn session(&self) -> AuthSession {
        AuthSession::new(&self.name, self.uuid, &self.access_token).with_provider(Provider::ElyBy)
    }

    /// `authenticate` and `refresh` answer `accessToken`, `clientToken` and
    /// `selectedProfile {id, name}`.
    fn from_grant(body: &str) -> Result<Self, Error> {
        let bad = || Error::Other("Ely.by sent an answer the game can't read".into());
        let v: serde_json::Value = serde_json::from_str(body).map_err(|_| bad())?;
        let profile = v.get("selectedProfile").ok_or_else(|| {
            Error::Other("This Ely.by account has no profile (choose a username on ely.by)".into())
        })?;
        let text = |v: &serde_json::Value, key: &str| {
            v.get(key).and_then(|s| s.as_str()).map(str::to_owned)
        };
        let name = text(profile, "name").ok_or_else(bad)?;
        if name.is_empty() || name.chars().count() > 16 {
            return Err(Error::Other(format!(
                "The Ely.by username {name:?} is not a valid player name"
            )));
        }
        Ok(Self {
            name,
            uuid: text(profile, "id")
                .and_then(|id| parse_uuid(&id))
                .ok_or_else(bad)?,
            access_token: text(&v, "accessToken").ok_or_else(bad)?,
            client_token: text(&v, "clientToken").ok_or_else(bad)?,
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "name": self.name,
            "uuid": format!("{:032x}", self.uuid),
            "access_token": self.access_token,
            "client_token": self.client_token,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|_| "not valid JSON".to_owned())?;
        let field = |k: &str| {
            v.get(k)
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("no {k}"))
        };
        let name = field("name")?;
        if name.chars().count() > 16 {
            return Err("name is too long".into());
        }
        Ok(Self {
            name: name.to_owned(),
            uuid: parse_uuid(field("uuid")?).ok_or("no valid uuid")?,
            access_token: field("access_token")?.to_owned(),
            client_token: field("client_token")?.to_owned(),
        })
    }

    /// `validate`: Ok(true) the token works, Ok(false) it was refused,
    /// Err(Unavailable) no answer (so nothing is known).
    pub fn validate(&self) -> Result<bool, Error> {
        match agent()
            .post(&format!("{}/auth/validate", base_url()))
            .set("Content-Type", "application/json; charset=utf-8")
            .send_string(&serde_json::json!({ "accessToken": self.access_token }).to_string())
        {
            Ok(_) => Ok(true),
            Err(ureq::Error::Status(status, _)) if status < 500 => Ok(false),
            Err(_) => Err(Error::Unavailable),
        }
    }

    /// `refresh`: a new access token for the same client token.
    pub fn refresh(&self) -> Result<Self, Error> {
        let body = post(
            "/auth/refresh",
            serde_json::json!({
                "accessToken": self.access_token,
                "clientToken": self.client_token,
                "requestUser": false,
            }),
        )
        .map_err(|e| match e {
            Error::InvalidCredentials | Error::TwoFactorRequired => Error::SessionExpired,
            other => other,
        })?;
        Self::from_grant(&body)
    }

    /// `invalidate`: the token stops working. Best effort: the player is
    /// signed out here whatever the answer.
    pub fn sign_out(&self) {
        let _ = post(
            "/auth/invalidate",
            serde_json::json!({
                "accessToken": self.access_token,
                "clientToken": self.client_token,
            }),
        );
    }
}

/// The two-factor code goes after the password, `password:code`.
fn login_password(password: &str, two_factor: Option<&str>) -> String {
    match two_factor.map(str::trim).filter(|c| !c.is_empty()) {
        Some(code) => format!("{password}:{code}"),
        None => password.to_owned(),
    }
}

/// Sign in with the account's login (username or e-mail) and password. An
/// account with two-factor authentication answers `TwoFactorRequired` until
/// the code comes along.
pub fn authenticate(
    login: &str,
    password: &str,
    two_factor: Option<&str>,
) -> Result<Account, Error> {
    let mut random = [0u8; 16];
    horizon_protocol::crypto::random_bytes(&mut random).map_err(|e| Error::Other(e.to_string()))?;
    let body = post(
        "/auth/authenticate",
        serde_json::json!({
            "username": login.trim(),
            "password": login_password(password, two_factor),
            "clientToken": format!("{:032x}", u128::from_be_bytes(random)),
            "requestUser": false,
        }),
    )?;
    Account::from_grant(&body)
}

/// Make a kept account usable: its token checked, renewed when refused.
pub fn restore(account: Account) -> Result<Account, Error> {
    if account.validate()? {
        Ok(account)
    } else {
        account.refresh()
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(&format!("Horizon/{}", horizon_core::release::VERSION))
        .build()
}

fn post(path: &str, body: serde_json::Value) -> Result<String, Error> {
    match agent()
        .post(&format!("{}{path}", base_url()))
        .set("Content-Type", "application/json; charset=utf-8")
        .send_string(&body.to_string())
    {
        Ok(response) => response.into_string().map_err(|_| Error::Unavailable),
        Err(ureq::Error::Status(status, response)) => Err(map_error(
            status,
            &response.into_string().unwrap_or_default(),
        )),
        Err(ureq::Error::Transport(_)) => Err(Error::Unavailable),
    }
}

/// The authserver's `{"error", "errorMessage"}` bodies.
fn map_error(status: u16, body: &str) -> Error {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let message = v.get("errorMessage").and_then(|m| m.as_str()).unwrap_or("");
    if status >= 500 {
        return Error::Unavailable;
    }
    if message.to_ascii_lowercase().contains("two factor") {
        return Error::TwoFactorRequired;
    }
    if status == 401
        || v.get("error").and_then(|e| e.as_str()) == Some("ForbiddenOperationException")
    {
        return Error::InvalidCredentials;
    }
    if message.is_empty() {
        Error::Other(format!("Status: {status}"))
    } else {
        Error::Other(message.to_owned())
    }
}

/// The account kept in `dir`, if any (a damaged file counts as none).
pub fn load(dir: &Path) -> Option<Account> {
    let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
    match Account::from_json(&text) {
        Ok(account) => Some(account),
        Err(why) => {
            log::warn!("Horizon: ignoring {FILE}: {why}");
            None
        }
    }
}

/// Keep the account in `dir`: written to a new file only its owner can read,
/// then moved over the old one.
pub fn save(dir: &Path, account: &Account) -> std::io::Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(account.to_json().as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, dir.join(FILE))
}

/// Forget the kept account (the token is also invalidated by `sign_out`).
pub fn forget(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(FILE));
}

/// The kept account, ready for this run. No file: `Ok(None)`. No network
/// keeps the token as it is (joining then says the servers are unreachable);
/// a token that was refused and can't be renewed is an error: sign in again.
pub fn stored_for_start(dir: &Path) -> Result<Option<Account>, Error> {
    let Some(kept) = load(dir) else {
        return Ok(None);
    };
    match restore(kept.clone()) {
        Ok(fresh) => {
            if fresh.access_token != kept.access_token
                && let Err(e) = save(dir, &fresh)
            {
                log::warn!("Horizon: could not keep the renewed Ely.by session: {e}");
            }
            Ok(Some(fresh))
        }
        Err(Error::Unavailable) => Ok(Some(kept)),
        Err(e) => Err(e),
    }
}

/// `--elyby-login`: sign in from the terminal and keep the account. The
/// password comes from `HORIZON_ELYBY_PASSWORD` or a prompt (shown as typed:
/// pipe it in if that matters), the two-factor code from `HORIZON_ELYBY_CODE`
/// or a prompt when the account asks for one.
pub fn cli_sign_in(login: &str, dir: &Path) -> Result<Account, String> {
    use std::io::BufRead;
    let ask = |prompt: &str| -> Result<String, String> {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    };
    let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let password = match env("HORIZON_ELYBY_PASSWORD") {
        Some(p) => p,
        None => ask(&format!("Ely.by password for {login}: "))?,
    };
    let mut code = env("HORIZON_ELYBY_CODE");
    let account = loop {
        match authenticate(login, &password, code.as_deref()) {
            Ok(account) => break account,
            Err(Error::TwoFactorRequired) if code.is_none() => {
                code = Some(ask("Two-factor code: ")?);
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    save(dir, &account).map_err(|e| {
        format!(
            "could not keep the Ely.by session in {}: {e}",
            dir.display()
        )
    })?;
    Ok(account)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: u128 = 0x069a79f444e94726a5befca90e38aaf5;
    const GRANT: &str = r#"{"accessToken":"acc","clientToken":"cli","availableProfiles":[{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch"}],"selectedProfile":{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch"}}"#;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("horizon-elyby-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn grants_become_accounts() {
        let a = Account::from_grant(GRANT).unwrap();
        assert_eq!((a.name.as_str(), a.uuid), ("Notch", UUID));
        let s = a.session();
        assert_eq!(s.provider, Provider::ElyBy);
        assert_eq!(s.uuid, UUID);
        // No profile, a bad id, a name Minecraft can't carry, a broken body.
        assert!(Account::from_grant(r#"{"accessToken":"a","clientToken":"c"}"#).is_err());
        assert!(
            Account::from_grant(
                r#"{"accessToken":"a","clientToken":"c","selectedProfile":{"id":"nope","name":"N"}}"#
            )
            .is_err()
        );
        let long = GRANT.replace("\"name\":\"Notch\"", "\"name\":\"ThisNameIsTooLong17\"");
        assert!(Account::from_grant(&long).is_err());
        assert!(Account::from_grant("").is_err());
    }

    #[test]
    fn errors_map_like_the_authserver_talks() {
        let two = r#"{"error":"ForbiddenOperationException","errorMessage":"Account protected with two factor auth."}"#;
        assert_eq!(map_error(401, two), Error::TwoFactorRequired);
        let bad = r#"{"error":"ForbiddenOperationException","errorMessage":"Invalid credentials. Invalid email or password."}"#;
        assert_eq!(map_error(401, bad), Error::InvalidCredentials);
        assert_eq!(map_error(401, ""), Error::InvalidCredentials);
        assert_eq!(map_error(503, "down"), Error::Unavailable);
        assert_eq!(
            map_error(
                400,
                r#"{"error":"IllegalArgumentException","errorMessage":"Bad"}"#
            ),
            Error::Other("Bad".into())
        );
        assert_eq!(map_error(400, ""), Error::Other("Status: 400".into()));
    }

    #[test]
    fn two_factor_code_follows_the_password() {
        assert_eq!(login_password("pw", None), "pw");
        assert_eq!(login_password("pw", Some("")), "pw");
        assert_eq!(login_password("pw", Some(" 123456 ")), "pw:123456");
    }

    #[test]
    fn base_url_override() {
        assert_eq!(normalize_base(None), DEFAULT_BASE);
        assert_eq!(normalize_base(Some("  ")), DEFAULT_BASE);
        assert_eq!(
            normalize_base(Some("http://127.0.0.1:9/")),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn accounts_round_trip_and_hide_their_tokens() {
        let a = Account::from_grant(GRANT).unwrap();
        let back = Account::from_json(&a.to_json()).unwrap();
        assert_eq!((back.name.as_str(), back.uuid), ("Notch", UUID));
        assert_eq!(back.access_token, "acc");
        assert_eq!(back.client_token, "cli");
        let shown = format!("{a:?} {:?}", a.session());
        assert!(!shown.contains("acc") && !shown.contains("cli"), "{shown}");
        assert!(Account::from_json("{}").is_err());
        assert!(Account::from_json("nope").is_err());
    }

    #[test]
    fn the_kept_account_is_private_and_replaceable() {
        let d = dir("kept");
        assert!(load(&d).is_none());
        let a = Account::from_grant(GRANT).unwrap();
        save(&d, &a).unwrap();
        assert_eq!(load(&d).unwrap().uuid, UUID);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(d.join(FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        let mut b = a.clone();
        b.access_token = "newer".into();
        save(&d, &b).unwrap();
        assert_eq!(load(&d).unwrap().access_token, "newer");
        assert!(!d.join(format!("{FILE}.tmp")).exists());
        std::fs::write(d.join(FILE), "garbage").unwrap();
        assert!(load(&d).is_none());
        forget(&d);
        assert!(!d.join(FILE).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
