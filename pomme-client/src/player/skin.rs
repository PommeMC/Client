//! A player's own skin: looked up from Mojang by name, by profile id or from
//! a profile's `textures` property, downloaded, and turned into the 64x64
//! sheet the renderer uploads.

/// Decoded skin ready for upload: always a 64x64 RGBA sheet (legacy 64x32
/// skins are converted), plus the profile's arm model.
pub(crate) struct SkinData {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub slim: bool,
}

pub(crate) async fn fetch_skin_texture_by_name(name: &str) -> Result<SkinData, String> {
    #[derive(serde::Deserialize)]
    struct NamedProfile {
        id: String,
    }

    // The name goes in a path segment, so it is checked and encoded rather
    // than pasted into the URL.
    if !super::valid_player_name(name) {
        return Err(format!("invalid player name {name:?}"));
    }
    let mut url = reqwest::Url::parse("https://api.mojang.com/users/profiles/minecraft/")
        .map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "profile url cannot take a path".to_owned())?
        .pop_if_empty()
        .push(name);
    let profile: NamedProfile = fetch_profile(url, name).await?;
    fetch_skin_texture(&profile.id).await
}

/// The profile of `who` that Mojang serves at `url`, which answers an unknown
/// player with no content or not found.
async fn fetch_profile<T: serde::de::DeserializeOwned>(
    url: impl reqwest::IntoUrl,
    who: &str,
) -> Result<T, String> {
    let response = crate::net::http::get(url)
        .await
        .map_err(pomme_auth::error_chain)?;
    if matches!(
        response.status(),
        reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::NOT_FOUND
    ) {
        return Err(format!("no profile for {who}"));
    }
    response
        .error_for_status()
        .map_err(pomme_auth::error_chain)?
        .json()
        .await
        .map_err(pomme_auth::error_chain)
}

pub(crate) async fn fetch_skin_texture(uuid: &str) -> Result<SkinData, String> {
    #[derive(serde::Deserialize)]
    struct SessionProfile {
        properties: Vec<ProfileProperty>,
    }
    #[derive(serde::Deserialize)]
    struct ProfileProperty {
        name: Option<String>,
        value: String,
    }

    let url = format!("https://sessionserver.mojang.com/session/minecraft/profile/{uuid}");
    let profile: SessionProfile = fetch_profile(url, uuid).await?;

    let value = &profile
        .properties
        .iter()
        .find(|p| p.name.as_deref() == Some("textures"))
        // TODO: authlib reads only the `textures` property; drop this fallback.
        .or_else(|| profile.properties.first())
        .ok_or("No properties")?
        .value;

    fetch_skin_texture_from_profile_property(value).await
}

pub(crate) async fn fetch_skin_texture_from_profile_property(
    value: &str,
) -> Result<SkinData, String> {
    let (skin_url, slim) = skin_url_from_texture_property(value)?;
    let (pixels, width, height) = fetch_skin_image(&skin_url).await?;
    Ok(SkinData {
        pixels,
        width,
        height,
        slim,
    })
}

fn skin_url_from_texture_property(value: &str) -> Result<(String, bool), String> {
    #[derive(serde::Deserialize)]
    struct TexturesPayload {
        textures: Textures,
    }
    struct Textures(Vec<(String, Texture)>);
    #[derive(serde::Deserialize)]
    struct Texture {
        url: Option<String>,
        metadata: Option<SkinMetadata>,
    }
    #[derive(serde::Deserialize)]
    struct SkinMetadata {
        model: Option<String>,
    }

    /// Gson's map adapter fails on a repeated key. Keys are authlib's texture
    /// `Type`, so unknown names all collide as `null`.
    impl<'de> serde::Deserialize<'de> for Textures {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Textures;

                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a textures map")
                }

                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Textures, A::Error> {
                    fn slot(key: &str) -> Option<&str> {
                        matches!(key, "SKIN" | "CAPE" | "ELYTRA").then_some(key)
                    }
                    let mut entries: Vec<(String, Texture)> = Vec::new();
                    while let Some((key, texture)) = map.next_entry::<String, Texture>()? {
                        if entries.iter().any(|(k, _)| slot(k) == slot(&key)) {
                            return Err(serde::de::Error::custom(format!("duplicate key: {key}")));
                        }
                        entries.push((key, texture));
                    }
                    Ok(Textures(entries))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .map_err(pomme_auth::error_chain)?;
    let payload: TexturesPayload =
        serde_json::from_slice(&decoded).map_err(pomme_auth::error_chain)?;

    // authlib `unpackTextures`: one bad url empties the whole payload.
    // TODO: vanilla also shows the default skin for other players whose
    // textures property isn't SIGNED (`PlayerInfo.createSkinLookup`); pomme
    // doesn't verify the signature yet.
    let mut skin = None;
    for (key, texture) in payload.textures.0 {
        match texture.url {
            Some(url) if is_allowed_texture_url(&url) => {
                if key == "SKIN" {
                    let slim = texture.metadata.and_then(|m| m.model).as_deref() == Some("slim");
                    skin = Some((url, slim));
                }
            }
            url => return Err(format!("texture url not allowed: {url:?}")),
        }
    }
    skin.ok_or_else(|| "No skin texture".to_string())
}

/// authlib's `TextureUrlChecker.isAllowedTextureDomain`: a `java.net.URI`
/// (`parse_untrusted_url`'s rules) with a case-sensitive http(s) scheme and a
/// host of exactly `textures.minecraft.net`.
fn is_allowed_texture_url(url: &str) -> bool {
    const HOST: &str = "textures.minecraft.net";
    if crate::chat_component::parse_untrusted_url(url.to_owned()).is_err() {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, ""));
    // `URI` scans the port as digits, then `Integer.parseInt`; an overflow
    // leaves no host.
    let port_ok = port.is_empty()
        || (port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<i32>().is_ok());
    matches!(scheme, "http" | "https") && host == HOST && port_ok
}

async fn fetch_skin_image(skin_url: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let skin_bytes = crate::net::http::get(skin_url)
        .await
        .map_err(pomme_auth::error_chain)?
        .error_for_status()
        .map_err(pomme_auth::error_chain)?
        .bytes()
        .await
        .map_err(pomme_auth::error_chain)?;

    let img = image::load_from_memory(&skin_bytes).map_err(pomme_auth::error_chain)?;
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();
    process_legacy_skin(rgba.into_raw(), w, h)
}

const SKIN_W: u32 = 64;

fn skin_px(x: u32, y: u32) -> usize {
    ((y * SKIN_W + x) * 4) as usize
}

/// `SkinTextureDownloader.processLegacySkin`: rejects bad sizes, upgrades
/// legacy 64x32 sheets to 64x64 by mirroring the right limbs into the modern
/// left-limb slots, and applies the alpha fixups.
fn process_legacy_skin(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    if width != 64 || (height != 32 && height != 64) {
        return Err(format!(
            "Discarding incorrectly sized ({width}x{height}) skin texture"
        ));
    }
    let legacy = height == 32;
    let mut img = if legacy {
        let mut full = vec![0u8; (SKIN_W * SKIN_W * 4) as usize];
        full[..pixels.len()].copy_from_slice(&pixels);
        full
    } else {
        pixels
    };

    if legacy {
        // (x, y, dx, dy, w, h): mirror the right leg/arm into the left slots.
        const COPIES: [(u32, u32, i32, i32, u32, u32); 12] = [
            (4, 16, 16, 32, 4, 4),
            (8, 16, 16, 32, 4, 4),
            (0, 20, 24, 32, 4, 12),
            (4, 20, 16, 32, 4, 12),
            (8, 20, 8, 32, 4, 12),
            (12, 20, 16, 32, 4, 12),
            (44, 16, -8, 32, 4, 4),
            (48, 16, -8, 32, 4, 4),
            (40, 20, 0, 32, 4, 12),
            (44, 20, -8, 32, 4, 12),
            (48, 20, -16, 32, 4, 12),
            (52, 20, -8, 32, 4, 12),
        ];
        for (x, y, dx, dy, w, h) in COPIES {
            copy_rect_mirrored(&mut img, x, y, dx, dy, w, h);
        }
    }
    set_no_alpha(&mut img, 0, 0, 32, 16);
    if legacy {
        strip_alpha_if_opaque(&mut img, 32, 0, 64, 32);
    }
    set_no_alpha(&mut img, 0, 16, 64, 32);
    set_no_alpha(&mut img, 16, 48, 48, 64);
    Ok((img, SKIN_W, SKIN_W))
}

/// `NativeImage.copyRect(x, y, dx, dy, w, h, true, false)`: copies the rect at
/// (x, y) to (x + dx, y + dy) with each row written right-to-left.
fn copy_rect_mirrored(img: &mut [u8], x: u32, y: u32, dx: i32, dy: i32, w: u32, h: u32) {
    for row in 0..h {
        for col in 0..w {
            let src = skin_px(x + col, y + row);
            let dst_x = (x as i32 + dx) as u32 + (w - 1 - col);
            let dst_y = (y as i32 + dy) as u32 + row;
            let dst = skin_px(dst_x, dst_y);
            img.copy_within(src..src + 4, dst);
        }
    }
}

/// Forces the rect fully opaque (base layer regions never carry transparency).
fn set_no_alpha(img: &mut [u8], x0: u32, y0: u32, x1: u32, y1: u32) {
    for y in y0..y1 {
        for x in x0..x1 {
            img[skin_px(x, y) + 3] = 0xFF;
        }
    }
}

/// The "Notch transparency hack": a fully opaque hat region predates hat
/// transparency, so strip its alpha entirely instead of drawing a solid box.
fn strip_alpha_if_opaque(img: &mut [u8], x0: u32, y0: u32, x1: u32, y1: u32) {
    let all_opaque = (y0..y1).all(|y| (x0..x1).all(|x| img[skin_px(x, y) + 3] >= 128));
    if all_opaque {
        for y in y0..y1 {
            for x in x0..x1 {
                img[skin_px(x, y) + 3] = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_skin_url_from_textures_property() {
        let classic = textures_property(
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/testskin"}}}"#,
        );
        let slim = textures_property(
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/testskin","metadata":{"model":"slim"}}}}"#,
        );
        for (value, is_slim) in [
            (classic.as_str(), false),
            (classic.trim_end_matches('='), false),
            (slim.as_str(), true),
        ] {
            assert_eq!(
                skin_url_from_texture_property(value).unwrap(),
                (
                    "https://textures.minecraft.net/texture/testskin".to_owned(),
                    is_slim
                ),
                "{value}"
            );
        }
    }

    fn textures_property(payload: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(payload)
    }

    #[test]
    fn texture_urls_follow_authlib_allow_list() {
        for ok in [
            "https://textures.minecraft.net/texture/abc",
            "http://textures.minecraft.net/texture/abc",
            "https://textures.minecraft.net:443/texture/abc",
        ] {
            assert!(is_allowed_texture_url(ok), "{ok}");
        }
        for bad in [
            "https://example.com/texture/abc",
            "https://textures.minecraft.net.evil.com/abc",
            "https://evil.com/textures.minecraft.net",
            "https://textures.minecraft.net@evil.com/abc",
            "https://evil.com#@textures.minecraft.net",
            "https://TEXTURES.minecraft.net/texture/abc",
            "HTTPS://textures.minecraft.net/texture/abc",
            "ftp://textures.minecraft.net/texture/abc",
            "file:///etc/passwd",
            "http://192.168.0.1/skin.png",
            "https://textures.minecraft.net/texture/a b",
            "https://textures.minecraft.net/texture/a|b",
            r"https://textures.minecraft.net\@evil.com/abc",
            "https://textures.minecraft.net:abc/texture/abc",
            "https://textures.minecraft.net:99999999999/texture/abc",
            "not a url",
            "",
        ] {
            assert!(!is_allowed_texture_url(bad), "{bad}");
        }
    }

    #[test]
    fn foreign_urls_and_duplicate_keys_reject_the_whole_payload() {
        for payload in [
            r#"{"textures":{"SKIN":{"url":"http://192.168.0.1/skin.png"}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"CAPE":{"url":"http://192.168.0.1/cape.png"}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"ELYTRA":{}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"SKIN":{"url":"https://textures.minecraft.net/texture/b"}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"FOO":{"url":"https://textures.minecraft.net/texture/b"},"BAR":{"url":"https://textures.minecraft.net/texture/c"}}}"#,
        ] {
            let value = textures_property(payload);
            assert!(skin_url_from_texture_property(&value).is_err(), "{payload}");
        }
    }

    fn set_px(img: &mut [u8], x: u32, y: u32, rgba: [u8; 4]) {
        img[skin_px(x, y)..skin_px(x, y) + 4].copy_from_slice(&rgba);
    }

    fn get_px(img: &[u8], x: u32, y: u32) -> [u8; 4] {
        img[skin_px(x, y)..skin_px(x, y) + 4].try_into().unwrap()
    }

    #[test]
    fn rejects_incorrectly_sized_skins() {
        assert!(process_legacy_skin(vec![0; 128 * 128 * 4], 128, 128).is_err());
        assert!(process_legacy_skin(vec![0; 64 * 16 * 4], 64, 16).is_err());
    }

    #[test]
    fn passes_64x64_skins_through() {
        let mut img = vec![0u8; 64 * 64 * 4];
        set_px(&mut img, 20, 50, [1, 2, 3, 200]);
        let (out, w, h) = process_legacy_skin(img, 64, 64).unwrap();
        assert_eq!((w, h), (64, 64));
        // Base region alpha is forced opaque, rgb untouched.
        assert_eq!(get_px(&out, 20, 50), [1, 2, 3, 255]);
    }

    #[test]
    fn converts_legacy_skins_to_64x64() {
        let mut img = vec![0u8; 64 * 32 * 4];
        // Right leg front, top-left pixel: (4, 20).
        set_px(&mut img, 4, 20, [10, 20, 30, 255]);
        // Right arm front, top-left pixel: (44, 20).
        set_px(&mut img, 44, 20, [40, 50, 60, 255]);
        let (out, w, h) = process_legacy_skin(img, 64, 32).unwrap();
        assert_eq!((w, h), (64, 64));

        // copyRect(4, 20, 16, 32, 4, 12, mirrored): left leg front spans
        // x 20..24, and mirroring puts the source's left edge on the right.
        assert_eq!(get_px(&out, 23, 52), [10, 20, 30, 255]);
        // copyRect(44, 20, -8, 32, 4, 12, mirrored): left arm front x 36..40.
        assert_eq!(get_px(&out, 39, 52), [40, 50, 60, 255]);
    }

    #[test]
    fn strips_alpha_of_fully_opaque_legacy_hat() {
        let mut img = vec![0u8; 64 * 32 * 4];
        for y in 0..32 {
            for x in 32..64 {
                set_px(&mut img, x, y, [9, 9, 9, 255]);
            }
        }
        let (out, _, _) = process_legacy_skin(img, 64, 32).unwrap();
        assert_eq!(get_px(&out, 40, 8)[3], 0);
    }
}
