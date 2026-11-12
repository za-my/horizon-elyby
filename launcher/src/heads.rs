//! Players' heads for the Servers page: the face of the skin the game shows for them. Without the skin
//! service a player has vanilla's default skin for their UUID (`DefaultPlayerSkin.get`), read from the
//! player's own prepared game files.
use std::path::Path;

/// `DefaultPlayerSkin.get(UUID)`: the default skin by `UUID.hashCode` (as the game's tab list picks it).
pub fn default_skin(uuid: u128) -> &'static str {
    const SKINS: [&str; 18] = [
        "slim/alex",
        "slim/ari",
        "slim/efe",
        "slim/kai",
        "slim/makena",
        "slim/noor",
        "slim/steve",
        "slim/sunny",
        "slim/zuri",
        "wide/alex",
        "wide/ari",
        "wide/efe",
        "wide/kai",
        "wide/makena",
        "wide/noor",
        "wide/steve",
        "wide/sunny",
        "wide/zuri",
    ];
    let hilo = ((uuid >> 64) as u64) ^ (uuid as u64);
    let hash = ((hilo >> 32) as i32) ^ (hilo as i32);
    SKINS[hash.rem_euclid(SKINS.len() as i32) as usize]
}

/// A UUID written with or without dashes.
pub fn parse_uuid(text: &str) -> Option<u128> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32).then(|| u128::from_str_radix(&hex, 16).ok())?
}

/// The 8 by 8 face, row by row as RGBA, with the hat layer over it: what the tab list draws.
pub fn face(skin_png: &[u8]) -> Option<Vec<[u8; 4]>> {
    let skin = image::load_from_memory_with_format(skin_png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    if skin.width() < 64 || skin.height() < 32 {
        return None;
    }
    let mut out = Vec::with_capacity(64);
    for y in 0..8 {
        for x in 0..8 {
            let base = skin.get_pixel(8 + x, 8 + y).0;
            let hat = skin.get_pixel(40 + x, 8 + y).0;
            out.push(if hat[3] > 0 { hat } else { base });
        }
    }
    Some(out)
}

/// The default skin a player with `uuid` wears, as a file of the prepared game files in
/// `resources`, and whether its arms are slim.
pub fn default_skin_file(resources: &Path, uuid: &str) -> Option<(std::path::PathBuf, bool)> {
    let skin = default_skin(parse_uuid(uuid)?);
    let path = resources
        .join("assets/minecraft/textures/entity/player")
        .join(format!("{skin}.png"));
    Some((path, skin.starts_with("slim/")))
}

/// The face of a player with a default skin, from the prepared game files in `resources`.
pub fn default_face(resources: &Path, uuid: &str) -> Option<Vec<[u8; 4]>> {
    face(&std::fs::read(default_skin_file(resources, uuid)?.0).ok()?)
}

fn profile_pixels(png: &[u8]) -> Option<Vec<[u8; 4]>> {
    // Check dimensions before decoding a downloaded image; skin heads need no large allocation.
    let dimensions =
        image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png)
            .into_dimensions()
            .ok()?;
    matches!(dimensions, (64, 32 | 64))
        .then(|| face(png))
        .flatten()
}

/// Path in `cache` where `profile_face` stores the texture PNG for `url`.
pub fn cached_skin_path(url: &str, cache: &Path) -> std::path::PathBuf {
    use sha1::{Digest, Sha1};
    cache.join(format!("{:x}.png", Sha1::digest(url.as_bytes())))
}

/// The whole skin PNG `profile_face` kept for `url` in `cache`, if it fetched it.
pub fn cached_skin(url: &str, cache: &Path) -> Option<Vec<u8>> {
    std::fs::read(cached_skin_path(url, cache)).ok()
}

/// Public Minecraft and Ely.by texture URLs only; never send credentials to a skin host.
pub(crate) fn is_valid_texture_url(url: &str) -> bool {
    if let Some(texture) = url
        .strip_prefix("https://textures.minecraft.net/texture/")
        .or_else(|| url.strip_prefix("http://textures.minecraft.net/texture/"))
    {
        return !texture.is_empty() && texture.bytes().all(|b| b.is_ascii_hexdigit());
    }
    if let Some(rest) = url
        .strip_prefix("http://ely.by/storage/skins/")
        .or_else(|| url.strip_prefix("https://ely.by/storage/skins/"))
        .or_else(|| url.strip_prefix("http://ely.by/storage/cloaks/"))
        .or_else(|| url.strip_prefix("https://ely.by/storage/cloaks/"))
        .or_else(|| url.strip_prefix("http://skinsystem.ely.by/textures/"))
        .or_else(|| url.strip_prefix("https://skinsystem.ely.by/textures/"))
        .or_else(|| url.strip_prefix("http://skinsystem.ely.by/cloaks/"))
        .or_else(|| url.strip_prefix("https://skinsystem.ely.by/cloaks/"))
    {
        let hash = rest.strip_suffix(".png").unwrap_or(rest);
        return !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit());
    }
    false
}

fn fetch_texture_bytes(url: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let response = crate::proxy::agent(
        ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)),
        url,
    )
    .build()
    .get(url)
    .call()
    .ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// Returns a local PNG path in `cache` for a valid skin or cape `url`, downloading it if missing.
pub fn cache_skin_file(url: &str, cache: &Path, is_cape: bool) -> Option<std::path::PathBuf> {
    cache_skin_file_with(url, cache, is_cape, &fetch_texture_bytes)
}

pub(crate) fn cache_skin_file_with(
    url: &str,
    cache: &Path,
    is_cape: bool,
    fetch: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Option<std::path::PathBuf> {
    if !is_valid_texture_url(url) {
        return None;
    }
    let valid = |bytes: &[u8]| {
        if is_cape {
            crate::skins::read_cape(bytes).is_ok()
        } else {
            crate::skins::read_skin(bytes).is_ok()
        }
    };
    let file = cached_skin_path(url, cache);
    if std::fs::read(&file).ok().is_some_and(|b| valid(&b)) {
        return Some(file);
    }
    let bytes = fetch(url)?;
    if !valid(&bytes) {
        return None;
    }
    std::fs::create_dir_all(cache).ok()?;
    std::fs::write(&file, bytes).ok()?;
    Some(file)
}

/// A signed-in profile's public skin, cached independently of its authentication tokens.
/// This may do disk/network I/O and must run on a worker. Missing skins use the UUID default;
/// the `bool` says the download failed, so the real skin is worth asking for again later.
pub fn profile_face(
    account: &crate::account::Account,
    cache: &Path,
    resources: &Path,
) -> (Option<Vec<[u8; 4]>>, bool) {
    let failed = std::cell::Cell::new(false);
    let face = profile_face_with(account, cache, resources, &|url| {
        failed.set(true);
        let bytes = fetch_texture_bytes(url)?;
        failed.set(false);
        Some(bytes)
    });
    (face, failed.get())
}

/// How long a skin URL the session server gave is trusted before it is asked again (a skin
/// changed on minecraft.net shows within a day).
const SESSION_SKIN_FOR: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// The skin URL Mojang's public session server lists for the profile `uuid` (32 hex digits), kept
/// beside the cached skins for [`SESSION_SKIN_FOR`]. None when it has none (the default skin) or
/// can't be reached.
fn session_skin(
    uuid: &str,
    cache: &Path,
    fetch: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Option<String> {
    use base64::Engine;
    if uuid.len() != 32 || !uuid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let file = cache.join(format!("{uuid}.skin-url"));
    let fresh = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < SESSION_SKIN_FOR);
    if fresh {
        if let Ok(url) = std::fs::read_to_string(&file) {
            return (!url.is_empty()).then_some(url);
        }
    }
    let body = fetch(&format!(
        "https://sessionserver.mojang.com/session/minecraft/profile/{uuid}"
    ))?;
    let profile: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let textures = profile["properties"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == "textures")?["value"]
        .as_str()?;
    let textures = base64::engine::general_purpose::STANDARD
        .decode(textures)
        .ok()?;
    let textures: serde_json::Value = serde_json::from_slice(&textures).ok()?;
    // No SKIN: the profile wears its default skin; remembered as such (empty) too.
    let url = textures["textures"]["SKIN"]["url"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if std::fs::create_dir_all(cache).is_ok() {
        let _ = std::fs::write(&file, &url);
    }
    (!url.is_empty()).then_some(url)
}

fn profile_face_with(
    account: &crate::account::Account,
    cache: &Path,
    resources: &Path,
    fetch: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Option<Vec<[u8; 4]>> {
    // An account saved without its skin (signed in before skins were kept, or its profile
    // answered without one) asks Mojang's session server, which needs no token.
    let url = account
        .skin
        .as_ref()
        .map(|(url, _)| url.clone())
        .or_else(|| match account.provider {
            crate::account::AccountProvider::Microsoft => session_skin(&account.uuid, cache, fetch),
            crate::account::AccountProvider::ElyBy => {
                crate::account::elyby_skin(&account.name).0.map(|(url, _)| url)
            }
        });
    if let Some(url) = &url {
        if is_valid_texture_url(url) {
            let file = cached_skin_path(url, cache);
            if let Some(face) = std::fs::read(&file)
                .ok()
                .and_then(|png| profile_pixels(&png))
            {
                return Some(face);
            }
            if let Some(png) = fetch(url) {
                if let Some(face) = profile_pixels(&png) {
                    if std::fs::create_dir_all(cache).is_ok() {
                        let _ = std::fs::write(file, png);
                    }
                    return Some(face);
                }
            }
        }
    }
    default_face(resources, &account.uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_skins_follow_the_uuid_hash() {
        // OfflinePlayer:Alex and OfflinePlayer:Steve, as vanilla's server makes them.
        assert_eq!(
            parse_uuid("36532b5e-c442-3dbb-a24c-c7e55d0f979a"),
            Some(0x36532b5ec4423dbba24cc7e55d0f979a)
        );
        assert_eq!(default_skin(0), "slim/alex");
        assert_eq!(default_skin(1), "slim/ari");
        assert_eq!(default_skin(u128::MAX), "slim/alex");
        assert!(parse_uuid("not-a-uuid").is_none());
    }

    #[test]
    fn the_face_wears_its_hat() {
        let mut skin = image::RgbaImage::new(64, 64);
        skin.put_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
        skin.put_pixel(9, 8, image::Rgba([4, 5, 6, 255]));
        skin.put_pixel(41, 8, image::Rgba([7, 8, 9, 255]));
        let mut png = Vec::new();
        skin.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let face = face(&png).unwrap();
        assert_eq!(face.len(), 64);
        assert_eq!(face[0], [1, 2, 3, 255]);
        assert_eq!(face[1], [7, 8, 9, 255]);
    }

    #[test]
    fn profile_heads_are_cached_by_skin_and_work_offline() {
        let dir = std::env::temp_dir().join(format!(
            "hz-profile-head-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut skin = image::RgbaImage::new(64, 64);
        skin.put_pixel(8, 8, image::Rgba([12, 34, 56, 255]));
        let mut png = Vec::new();
        skin.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut account = crate::account::Account {
            name: "TestPlayer".into(),
            uuid: "00000000000000000000000000000000".into(),
            skin: Some((
                "https://textures.minecraft.net/texture/abc".into(),
                "classic".into(),
            )),
            cape: None,
            provider: crate::account::AccountProvider::Microsoft,
        };
        let calls = std::cell::Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Some(png.clone())
        };
        let head = profile_face_with(&account, &dir, &dir, &fetch).unwrap();
        assert_eq!(head[0], [12, 34, 56, 255]);
        assert_eq!(
            profile_face_with(&account, &dir, &dir, &|_| panic!(
                "cached head must not need the network"
            )),
            Some(head)
        );
        account.skin.as_mut().unwrap().0 = "https://textures.minecraft.net/texture/def".into();
        assert!(profile_face_with(&account, &dir, &dir, &fetch).is_some());
        assert_eq!(calls.get(), 2, "skin changes must not reuse an old image");
        assert!(profile_pixels(&png).is_some());
        let mut oversized = Vec::new();
        image::RgbaImage::new(128, 64)
            .write_to(
                &mut std::io::Cursor::new(&mut oversized),
                image::ImageFormat::Png,
            )
            .unwrap();
        assert!(profile_pixels(&oversized).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_account_saved_without_its_skin_asks_the_session_server() {
        use base64::Engine;
        let dir = std::env::temp_dir().join(format!(
            "hz-profile-session-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut skin = image::RgbaImage::new(64, 64);
        skin.put_pixel(8, 8, image::Rgba([90, 80, 70, 255]));
        let mut png = Vec::new();
        skin.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let uuid = "069a79f444e94726a5befca90e38aaf5";
        let textures = base64::engine::general_purpose::STANDARD.encode(
            r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/ab12"}}}"#,
        );
        let session = format!(
            r#"{{"id":"{uuid}","name":"Notch","properties":[{{"name":"textures","value":"{textures}"}}]}}"#
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let fetch = |url: &str| {
            asked.borrow_mut().push(url.to_owned());
            if url.contains("sessionserver.mojang.com") {
                Some(session.clone().into_bytes())
            } else if url == "http://textures.minecraft.net/texture/ab12" {
                Some(png.clone())
            } else {
                None
            }
        };
        let account = crate::account::Account {
            name: "Notch".into(),
            uuid: uuid.into(),
            skin: None,
            cape: None,
            provider: crate::account::AccountProvider::Microsoft,
        };
        // No default skins in `resources`: the face can only be the session's skin.
        let none = dir.join("no-resources");
        let head = profile_face_with(&account, &dir, &none, &fetch).unwrap();
        assert_eq!(head[0], [90, 80, 70, 255]);
        assert_eq!(
            asked.borrow()[0],
            format!("https://sessionserver.mojang.com/session/minecraft/profile/{uuid}")
        );
        // Asked once: the URL and the skin are kept.
        asked.borrow_mut().clear();
        assert_eq!(profile_face_with(&account, &dir, &none, &fetch), Some(head));
        assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
        // Offline, with nothing kept: no face, so the launcher draws its user icon.
        let fresh = dir.join("fresh");
        assert!(profile_face_with(&account, &fresh, &none, &|_| None).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_profile_skin_uses_the_uuid_default() {
        let dir = std::env::temp_dir().join(format!("hz-profile-default-{}", std::process::id()));
        let path = dir.join("assets/minecraft/textures/entity/player/slim/alex.png");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut skin = image::RgbaImage::new(64, 64);
        skin.put_pixel(8, 8, image::Rgba([3, 5, 7, 255]));
        skin.save(&path).unwrap();
        let account = crate::account::Account {
            name: "TestPlayer".into(),
            uuid: "00000000000000000000000000000000".into(),
            skin: Some((
                "https://textures.minecraft.net/texture/abc".into(),
                "classic".into(),
            )),
            cape: None,
            provider: crate::account::AccountProvider::Microsoft,
        };
        let head = profile_face_with(&account, &dir.join("cache"), &dir, &|_| None).unwrap();
        assert_eq!(head[0], [3, 5, 7, 255]);
        let mut blocked = account;
        blocked.skin.as_mut().unwrap().0 = "https://example.com/not-a-minecraft-skin".into();
        assert!(
            profile_face_with(&blocked, &dir.join("cache"), &dir, &|_| panic!(
                "must not fetch an arbitrary profile URL"
            ))
            .is_some()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn elyby_skin_is_cached_and_drawn() {
        let dir = std::env::temp_dir().join(format!("hz-elyby-head-{}", std::process::id()));
        let mut skin = image::RgbaImage::new(64, 64);
        skin.put_pixel(8, 8, image::Rgba([77, 88, 99, 255]));
        let mut png = Vec::new();
        skin.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let account = crate::account::Account {
            name: "zamy".into(),
            uuid: "7f5647305a034ad8a675d4448385a413".into(),
            skin: Some(("http://ely.by/storage/skins/7d3901089c7e0736c95f5034afea51ee.png".into(), "classic".into())),
            cape: None,
            provider: crate::account::AccountProvider::ElyBy,
        };
        let head = profile_face_with(&account, &dir.join("cache"), &dir, &|_| Some(png.clone())).unwrap();
        assert_eq!(head[0], [77, 88, 99, 255]);
        let cached = cache_skin_file_with(
            "http://ely.by/storage/skins/7d3901089c7e0736c95f5034afea51ee.png",
            &dir.join("cache"),
            false,
            &|_| panic!("should reuse cached file"),
        )
        .unwrap();
        assert!(cached.is_file());
        assert!(cache_skin_file_with(
            "https://example.com/evil.png",
            &dir.join("cache"),
            false,
            &|_| panic!("must not fetch disallowed url"),
        )
        .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
