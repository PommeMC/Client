#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AssetId<'a> {
    pub namespace: &'a str,
    pub path: &'a str,
}

impl<'a> AssetId<'a> {
    pub fn parse(value: &'a str) -> Self {
        match value.split_once(':') {
            Some((namespace, path)) if !namespace.is_empty() && !path.is_empty() => {
                Self { namespace, path }
            }
            _ => Self {
                namespace: "minecraft",
                path: value,
            },
        }
    }

    pub fn asset_key(self, category: &str, suffix: &str) -> String {
        format!("{}/{category}/{}{}", self.namespace, self.path, suffix)
    }

    pub fn canonical(self) -> String {
        if self.namespace == "minecraft" {
            self.path.to_string()
        } else {
            format!("{}:{}", self.namespace, self.path)
        }
    }
}

/// `Identifier.toString` omits the default namespace, so ids written either
/// way compare equal once stripped.
pub(crate) fn strip_default_namespace(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

/// `Identifier` namespace (`path` false) or path characters; only the path
/// allows `/`.
pub(crate) const fn identifier_chars(text: &str, path: bool) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'-')
            || path && byte == b'/')
        {
            return false;
        }
        index += 1;
    }
    true
}

pub(crate) fn valid_asset_key(asset_key: &str) -> bool {
    let Some((namespace, path)) = asset_key.split_once('/') else {
        return false;
    };
    !matches!(namespace, "" | "." | "..")
        && identifier_chars(namespace, false)
        && identifier_chars(path, true)
        && !path
            .split('/')
            .any(|component| matches!(component, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_keys_reject_traversal_and_invalid_resource_location_syntax() {
        for valid in [
            "minecraft/font/default.json",
            "example/textures/font/custom.png",
            "example/font/subdir/test.ttf",
        ] {
            assert!(valid_asset_key(valid), "expected valid asset key: {valid}");
        }
        for invalid in [
            "../outside",
            "minecraft/../outside",
            "minecraft/font/../../outside",
            "minecraft/./font/default.json",
            "minecraft//font/default.json",
            "minecraft/\\outside",
            "/minecraft/font/default.json",
            "Minecraft/font/default.json",
            "minecraft/CAPS.ttf",
            "minecraft/",
        ] {
            assert!(
                !valid_asset_key(invalid),
                "accepted invalid asset key: {invalid}"
            );
        }
    }
}
