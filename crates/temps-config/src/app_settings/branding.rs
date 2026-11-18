// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! White-label console branding: product name, accent colour, logo and
//! favicon. Every field is optional; `None` keeps the built-in Temps look.
//!
//! Images are stored inline as base64 `data:` URIs so they need no file store
//! and travel with a settings backup. Only raster types are accepted: an SVG
//! can carry script, and this value is served to unauthenticated visitors.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Longest accepted product name, in characters.
pub const MAX_BRAND_NAME_CHARS: usize = 40;
/// Largest accepted logo, as the length of its `data:` URI in bytes.
pub const MAX_BRAND_LOGO_BYTES: usize = 128 * 1024;
/// Largest accepted favicon, as the length of its `data:` URI in bytes.
pub const MAX_BRAND_FAVICON_BYTES: usize = 32 * 1024;

const IMAGE_PREFIXES: [&str; 3] = [
    "data:image/png;base64,",
    "data:image/jpeg;base64,",
    "data:image/webp;base64,",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct BrandingSettings {
    /// Replaces "Temps" in the console chrome and the browser tab title.
    #[schema(example = "Acme Cloud")]
    pub product_name: Option<String>,
    /// Accent colour as `#rrggbb`; used for primary buttons and links.
    #[schema(example = "#2563eb")]
    pub primary_color: Option<String>,
    /// `data:image/png|jpeg|webp;base64,...` shown as the console mark.
    pub logo: Option<String>,
    /// `data:image/png|jpeg|webp;base64,...` used as the browser favicon.
    pub favicon: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BrandingError {
    #[error("branding.product_name is {len} characters; at most {max} are allowed")]
    NameTooLong { len: usize, max: usize },
    #[error("branding.product_name must not contain control characters")]
    NameHasControlCharacters,
    #[error("branding.primary_color '{value}' is not a #rrggbb hex colour")]
    InvalidColor { value: String },
    #[error(
        "branding.{field} must be a base64 data URI of a PNG, JPEG or WebP image \
         (SVG is refused because it can carry script)"
    )]
    UnsupportedImage { field: &'static str },
    #[error("branding.{field} is {size} bytes; at most {max} are allowed")]
    ImageTooLarge {
        field: &'static str,
        size: usize,
        max: usize,
    },
}

impl BrandingSettings {
    /// Validate and canonicalise: blanks become `None`, the name is trimmed
    /// and the colour lower-cased. Nothing is stored unless this succeeds.
    pub fn normalized(self) -> Result<Self, BrandingError> {
        Ok(Self {
            product_name: normalize_name(self.product_name)?,
            primary_color: normalize_color(self.primary_color)?,
            logo: normalize_image("logo", self.logo, MAX_BRAND_LOGO_BYTES)?,
            favicon: normalize_image("favicon", self.favicon, MAX_BRAND_FAVICON_BYTES)?,
        })
    }
}

fn blank_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_name(name: Option<String>) -> Result<Option<String>, BrandingError> {
    let Some(name) = blank_to_none(name) else {
        return Ok(None);
    };
    if name.chars().any(char::is_control) {
        return Err(BrandingError::NameHasControlCharacters);
    }
    let len = name.chars().count();
    if len > MAX_BRAND_NAME_CHARS {
        return Err(BrandingError::NameTooLong {
            len,
            max: MAX_BRAND_NAME_CHARS,
        });
    }
    Ok(Some(name))
}

fn normalize_color(color: Option<String>) -> Result<Option<String>, BrandingError> {
    let Some(color) = blank_to_none(color) else {
        return Ok(None);
    };
    let valid = color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err(BrandingError::InvalidColor { value: color });
    }
    Ok(Some(color.to_ascii_lowercase()))
}

fn normalize_image(
    field: &'static str,
    image: Option<String>,
    max: usize,
) -> Result<Option<String>, BrandingError> {
    let Some(image) = blank_to_none(image) else {
        return Ok(None);
    };
    if image.len() > max {
        return Err(BrandingError::ImageTooLarge {
            field,
            size: image.len(),
            max,
        });
    }
    let Some(payload) = IMAGE_PREFIXES
        .iter()
        .find_map(|prefix| image.strip_prefix(prefix))
    else {
        return Err(BrandingError::UnsupportedImage { field });
    };
    let is_base64 = !payload.is_empty()
        && payload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
    if !is_base64 {
        return Err(BrandingError::UnsupportedImage { field });
    }
    Ok(Some(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    #[test]
    fn blanks_become_none_and_values_are_canonicalised() {
        let branding = BrandingSettings {
            product_name: Some("  Acme Cloud  ".into()),
            primary_color: Some("#2563EB".into()),
            logo: Some(PNG.into()),
            favicon: Some("   ".into()),
        }
        .normalized()
        .unwrap();
        assert_eq!(branding.product_name.as_deref(), Some("Acme Cloud"));
        assert_eq!(branding.primary_color.as_deref(), Some("#2563eb"));
        assert_eq!(branding.logo.as_deref(), Some(PNG));
        assert_eq!(branding.favicon, None);
        assert_eq!(
            BrandingSettings::default().normalized().unwrap(),
            BrandingSettings::default()
        );
    }

    #[test]
    fn colours_must_be_six_digit_hex() {
        for bad in ["2563eb", "#25e", "#2563eg", "red", "#2563eb00"] {
            let result = BrandingSettings {
                primary_color: Some(bad.into()),
                ..Default::default()
            }
            .normalized();
            assert_eq!(
                result,
                Err(BrandingError::InvalidColor { value: bad.into() }),
                "{bad}"
            );
        }
    }

    #[test]
    fn names_are_bounded_and_free_of_control_characters() {
        let long = "x".repeat(MAX_BRAND_NAME_CHARS + 1);
        assert!(matches!(
            BrandingSettings {
                product_name: Some(long),
                ..Default::default()
            }
            .normalized(),
            Err(BrandingError::NameTooLong { len: 41, max: 40 })
        ));
        assert_eq!(
            BrandingSettings {
                product_name: Some("Acme\u{7}".into()),
                ..Default::default()
            }
            .normalized(),
            Err(BrandingError::NameHasControlCharacters)
        );
    }

    #[test]
    fn only_raster_base64_images_are_accepted() {
        for bad in [
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:image/png,rawbytes",
            "https://example.com/logo.png",
            "data:image/png;base64,<script>",
            "data:image/png;base64,",
        ] {
            assert_eq!(
                BrandingSettings {
                    logo: Some(bad.into()),
                    ..Default::default()
                }
                .normalized(),
                Err(BrandingError::UnsupportedImage { field: "logo" }),
                "{bad}"
            );
        }
        for ok in [
            PNG,
            "data:image/jpeg;base64,/9j/4AAQ",
            "data:image/webp;base64,UklGRg==",
        ] {
            assert!(BrandingSettings {
                favicon: Some(ok.into()),
                ..Default::default()
            }
            .normalized()
            .is_ok());
        }
    }

    #[test]
    fn oversized_images_are_refused_with_their_size() {
        let favicon = format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_BRAND_FAVICON_BYTES)
        );
        let size = favicon.len();
        assert_eq!(
            BrandingSettings {
                favicon: Some(favicon),
                ..Default::default()
            }
            .normalized(),
            Err(BrandingError::ImageTooLarge {
                field: "favicon",
                size,
                max: MAX_BRAND_FAVICON_BYTES,
            })
        );
    }
}
