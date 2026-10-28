//! Everything the offline `knot viz` page inlines: the vendored scripts, the
//! fonts as CSS and the brand images.

use base64::Engine;

pub const GRAPHOLOGY_JS: &str = include_str!("../../assets/viz/graphology.umd.min.js");
pub const SIGMA_JS: &str = include_str!("../../assets/viz/sigma.min.js");
/// graphology-library's UMD build; its global is `graphologyLibrary` and the
/// layout is `graphologyLibrary.layoutForceAtlas2`.
pub const FA2_JS: &str = include_str!("../../assets/viz/graphology-library.min.js");

pub const SANS: &[u8] = include_bytes!("../../assets/fonts/KnotSans-Regular.ttf");
pub const SANS_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/KnotSans-SemiBold.ttf");
pub const MONO: &[u8] = include_bytes!("../../assets/fonts/KnotMono-Regular.ttf");

pub const LOGO_LIGHT: &[u8] = include_bytes!("../../assets/logo-light.png");
pub const LOGO_DARK: &[u8] = include_bytes!("../../assets/logo-dark.png");
pub const MARK_LIGHT_64: &[u8] = include_bytes!("../../assets/mark-light-64.png");
pub const MARK_DARK_64: &[u8] = include_bytes!("../../assets/mark-dark-64.png");

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn data_uri_png(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", b64(bytes))
}

/// Three `@font-face` rules with the fonts inlined as base64 data URIs.
pub fn font_face_css() -> String {
    [
        ("Knot Sans", 400, SANS),
        ("Knot Sans", 600, SANS_SEMIBOLD),
        ("Knot Mono", 400, MONO),
    ]
    .iter()
    .map(|(family, weight, bytes)| {
        format!(
            "@font-face {{ font-family: \"{family}\"; font-weight: {weight}; font-style: normal; font-display: block; src: url(data:font/ttf;base64,{}) format(\"truetype\"); }}\n",
            b64(bytes)
        )
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_scripts_are_present() {
        for js in [GRAPHOLOGY_JS, SIGMA_JS, FA2_JS] {
            assert!(js.len() > 10_000);
        }
        assert!(GRAPHOLOGY_JS.contains("graphology"));
        assert!(SIGMA_JS.contains("Sigma"));
        assert!(FA2_JS.contains("forceAtlas2") || FA2_JS.contains("forceatlas2"));
    }

    #[test]
    fn font_css_uses_knot_names_only() {
        let css = font_face_css();
        assert!(css.contains("font-family: \"Knot Sans\""));
        assert!(css.contains("font-family: \"Knot Mono\""));
        assert!(!css.contains("Plex"));
    }

    #[test]
    fn data_uri_is_png() {
        assert!(data_uri_png(LOGO_LIGHT).starts_with("data:image/png;base64,"));
        assert!(data_uri_png(MARK_DARK_64).starts_with("data:image/png;base64,"));
    }
}
