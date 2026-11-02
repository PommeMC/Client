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

    pub fn resource_id(
        self,
        category: &str,
        suffix: &str,
    ) -> Result<crate::resources::ResourceId, crate::resources::ResourceError> {
        crate::resources::ResourceId::from_parts(
            self.namespace,
            &format!("{category}/{}{suffix}", self.path),
        )
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
pub(crate) fn identifier_chars(text: &str, path: bool) -> bool {
    text.bytes().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'-')
            || path && byte == b'/'
    })
}

pub fn load_image_bytes(bytes: &[u8]) -> Result<image::DynamicImage, image::ImageError> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .decode()
}

pub fn resource_content(
    resources: &dyn crate::resources::ResourceManager,
    id: &crate::resources::ResourceId,
) -> std::io::Result<crate::resources::ResourceContent> {
    resources
        .get_resource(id)?
        .map(|resource| resource.content().clone())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing resource {id}"),
            )
        })
}

pub fn load_resource_image(
    resources: &dyn crate::resources::ResourceManager,
    id: &crate::resources::ResourceId,
) -> Result<image::DynamicImage, image::ImageError> {
    use std::io::Read;
    let content = resource_content(resources, id).map_err(image::ImageError::IoError)?;
    let mut bytes = Vec::new();
    content
        .open()
        .map_err(|error| image::ImageError::IoError(std::io::Error::other(error)))?
        .read_to_end(&mut bytes)
        .map_err(image::ImageError::IoError)?;
    load_image_bytes(&bytes)
}
