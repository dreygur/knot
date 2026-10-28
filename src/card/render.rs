//! Draw the share card: fill an SVG template, then rasterise it to a 1200×675 PNG.

use super::stats::{CardStats, Trend};
use anyhow::Context;
use base64::Engine;
use resvg::{tiny_skia, usvg};
use skrifa::prelude::{FontRef, LocationRef, MetadataProvider, Size};
use std::sync::Arc;

const TEMPLATE: &str = include_str!("card.svg");
const LOGO: &[u8] = include_bytes!("../../assets/logo-light.png");
const SANS: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf");
const SANS_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf");

const WIDTH: u32 = 1200;
const HEIGHT: u32 = 675;
/// Left and right edges of the content.
const LEFT: f32 = 56.0;
const RIGHT: f32 = 1144.0;
const GREEN: &str = "#34d399";
const MINUS: char = '\u{2212}';
const MAX_CHIPS: usize = 3;
/// Chips never run past this x, so they keep clear of the repo link.
const CHIPS_END: f32 = 800.0;
const MAX_REPO_CHARS: usize = 40;

/// Everything the card shows.
pub struct CardView {
    pub stats: CardStats,
    /// The repo folder name, shown only for `--repo`.
    pub repo_name: Option<String>,
    pub agents: Vec<String>,
}

/// The card as a 1200×675 PNG. Only the embedded fonts are used, so every machine draws the same pixels.
pub fn render_png(view: &CardView) -> anyhow::Result<Vec<u8>> {
    let mut fonts = usvg::fontdb::Database::new();
    for font in [SANS, SANS_SEMIBOLD, MONO] {
        fonts.load_font_data(font.to_vec());
    }
    let options = usvg::Options {
        font_family: "IBM Plex Sans".into(),
        fontdb: Arc::new(fonts),
        ..Default::default()
    };
    let tree =
        usvg::Tree::from_str(&svg(view), &options).context("card template is not valid SVG")?;
    let mut pixmap =
        tiny_skia::Pixmap::new(WIDTH, HEIGHT).context("could not allocate the card")?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .context("could not encode the card as PNG")
}

/// The filled-in SVG template.
pub(crate) fn svg(view: &CardView) -> String {
    let s = &view.stats;
    let tokens_saved = short(s.tokens_saved);
    let less_to_read = match s.less_to_read {
        Some(f) => match (f * 100.0).round() as u64 {
            0 => "0%".to_string(),
            pct => format!("{MINUS}{pct}%"),
        },
        // Nothing was read before: no figure to claim
        None => "<tspan fill=\"#8b97a8\">\u{2014}</tspan>".to_string(),
    };
    // Second headline sits one gap right of the wider of the first number and its caption
    let caption = width(SANS, "tokens saved", 24.0) + 12.0 + width(MONO, "≈ estimated", 18.0);
    let less_x = LEFT + width(SANS_SEMIBOLD, &tokens_saved, 84.0).max(caption) + 80.0;

    let mut learning_count = format!("{} learnings", grouped(s.learnings.count));
    match s.learnings.minutes {
        0 => {}
        m @ 1..=59 => learning_count += &format!(" · {m} min"),
        m => learning_count += &format!(" · ~{} h", (m + 30) / 60),
    }

    let top_right = match &view.repo_name {
        Some(name) => {
            let name: String = name.chars().take(MAX_REPO_CHARS).collect();
            format!(
                "<tspan fill=\"#c3ccd8\">{}</tspan> · {}",
                esc(&name),
                esc(&s.label)
            )
        }
        None => esc(&s.label),
    };

    let logo = base64::engine::general_purpose::STANDARD.encode(LOGO);
    fill(
        TEMPLATE,
        &[
            ("logo", logo),
            ("top_right", top_right),
            ("tokens_saved", tokens_saved),
            ("less_to_read", less_to_read),
            ("less_x", format!("{less_x:.1}")),
            ("trend", trend(&s.trend)),
            ("graph_tokens", saved(s.graph.tokens)),
            ("graph_count", format!("{} answers", grouped(s.graph.count))),
            ("learning_tokens", saved(s.learnings.tokens)),
            ("learning_count", esc(&learning_count)),
            ("swap_tokens", saved(s.swaps.tokens)),
            ("swap_count", format!("{} runs", grouped(s.swaps.count))),
            ("chips", chips(&view.agents)),
        ],
    )
}

/// 999, 1.2k, 4.1M, 1.2B.
pub fn short(n: u64) -> String {
    const UNITS: [(f64, &str); 3] = [(1e3, "k"), (1e6, "M"), (1e9, "B")];
    if n < 1_000 {
        return n.to_string();
    }
    let mut unit = UNITS
        .iter()
        .rposition(|(size, _)| n as f64 >= *size)
        .unwrap_or(0);
    // 999,960 rounds to "1000.0k"; step up a unit instead
    if unit + 1 < UNITS.len() && (n as f64 / UNITS[unit].0 * 10.0).round() >= 10_000.0 {
        unit += 1;
    }
    let (size, suffix) = UNITS[unit];
    format!("{:.1}{suffix}", n as f64 / size)
}

/// A tile's figure: "−2.6M", or "0" when nothing was saved.
fn saved(tokens: u64) -> String {
    match tokens {
        0 => "0".into(),
        t => format!("{MINUS}{}", short(t)),
    }
}

/// 1931 → "1,931".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The trend's space, right-aligned beside the headline: a smoothed line of the running total, or a per-session figure.
fn trend(trend: &Trend) -> String {
    let (x0, y0, w, h) = (RIGHT - 360.0, 151.0, 360.0, 100.0);
    let days = match trend {
        Trend::Daily(days) => days,
        Trend::PerSession(tokens) => {
            return format!(
                "<text x=\"{RIGHT}\" y=\"244.6\" text-anchor=\"end\" font-family=\"IBM Plex Sans\" font-size=\"24\" fill=\"#8b97a8\">≈ <tspan font-weight=\"600\" fill=\"#e6edf3\">{}</tspan> tokens saved per session</text>",
                short(*tokens)
            );
        }
    };
    let mut total = 0u64;
    let mut running: Vec<u64> = days
        .iter()
        .map(|d| {
            total += d;
            total
        })
        .collect();
    if running.len() < 2 {
        running = vec![0, total];
    }
    let top = total.max(1) as f32;
    let step = w / (running.len() - 1) as f32;
    let points: Vec<(f32, f32)> = running
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                x0 + i as f32 * step,
                y0 + h - 8.0 - *v as f32 / top * (h - 16.0),
            )
        })
        .collect();

    let mut line = format!("M{:.1} {:.1}", points[0].0, points[0].1);
    for i in 0..points.len() - 1 {
        let (p0, p1, p2, p3) = (
            points[i.saturating_sub(1)],
            points[i],
            points[i + 1],
            points[(i + 2).min(points.len() - 1)],
        );
        // Catmull-Rom control points, kept between the segment's ends so the line never dips or overshoots
        let (lo, hi) = (p1.1.min(p2.1), p1.1.max(p2.1));
        let c1 = (
            p1.0 + (p2.0 - p0.0) / 6.0,
            (p1.1 + (p2.1 - p0.1) / 6.0).clamp(lo, hi),
        );
        let c2 = (
            p2.0 - (p3.0 - p1.0) / 6.0,
            (p2.1 - (p3.1 - p1.1) / 6.0).clamp(lo, hi),
        );
        line += &format!(
            " C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
            c1.0, c1.1, c2.0, c2.1, p2.0, p2.1
        );
    }
    let bottom = y0 + h;
    format!(
        "<path d=\"{line} L{RIGHT} {bottom} L{x0} {bottom}Z\" fill=\"url(#area)\"/>\n  <path d=\"{line}\" fill=\"none\" stroke=\"{GREEN}\" stroke-width=\"4\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
    )
}

/// "Runs in" chips: the first three agents, then "+N" for the rest. Nothing when there are no agents.
fn chips(agents: &[String]) -> String {
    let mut x = LEFT;
    let mut out = String::new();
    let mut shown = 0;
    for label in agents.iter().take(MAX_CHIPS) {
        let w = width(SANS, label, 22.0) + 40.0;
        // Leave room for the "+N" chip; the first chip always shows
        if x + w > CHIPS_END - 70.0 && shown > 0 {
            break;
        }
        out += &chip(x, w, label);
        x += w + 10.0;
        shown += 1;
    }
    let rest = agents.len() - shown;
    if rest > 0 {
        let label = format!("+{rest}");
        out += &chip(x, width(SANS, &label, 22.0) + 40.0, &label);
    }
    out
}

fn chip(x: f32, w: f32, label: &str) -> String {
    format!(
        "<rect x=\"{:.1}\" y=\"608\" width=\"{w:.1}\" height=\"38\" rx=\"19\" fill=\"none\" stroke=\"#2a3442\" stroke-width=\"2\"/><text x=\"{:.1}\" y=\"634.5\" font-family=\"IBM Plex Sans\" font-size=\"22\" fill=\"#c3ccd8\">{}</text>\n  ",
        x + 1.0,
        x + 20.0,
        esc(label)
    )
}

/// Advance width of `text` in `font` at `size` px (no kerning, which is close enough for spacing).
fn width(font: &[u8], text: &str, size: f32) -> f32 {
    let Ok(font) = FontRef::new(font) else {
        return 0.0;
    };
    let (map, metrics) = (
        font.charmap(),
        font.glyph_metrics(Size::new(size), LocationRef::default()),
    );
    text.chars()
        .filter_map(|c| map.map(c))
        .filter_map(|g| metrics.advance_width(g))
        .sum()
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Replace each `{{key}}` in one pass, so a value that itself contains `{{...}}` is left alone.
fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out += &rest[..start];
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let key = &after[..end];
        match values.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => out += v,
            None => out += &rest[start..start + 2 + end + 2],
        }
        rest = &after[end + 2..];
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::stats::{self, LearningTile, Tile, Trend};

    fn sample() -> CardView {
        CardView {
            stats: CardStats {
                tokens_saved: 4_100_000,
                less_to_read: Some(0.68),
                graph: Tile {
                    tokens: 2_600_000,
                    count: 1_931,
                },
                learnings: LearningTile {
                    count: 84,
                    tokens: 40_000,
                    minutes: 660,
                },
                swaps: Tile {
                    tokens: 1_500_000,
                    count: 1_592,
                },
                label: "last 30 days".into(),
                trend: Trend::Daily(vec![1, 4, 2, 9, 12, 30, 41]),
            },
            repo_name: None,
            agents: vec!["Claude Code".into(), "Codex".into(), "Cursor".into()],
        }
    }

    /// The SVG without the logo's base64, which could spell anything.
    fn drawn(view: &CardView) -> String {
        let svg = svg(view);
        let start = svg.find("data:image/png;base64,").unwrap();
        let end = start + svg[start..].find('"').unwrap();
        format!("{}{}", &svg[..start], &svg[end..])
    }

    fn png_size(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        (be(&png[16..20]), be(&png[20..24]))
    }

    #[test]
    fn png_is_1200_by_675() {
        let png = render_png(&sample()).unwrap();
        assert_eq!(png_size(&png), (1200, 675));
    }

    #[test]
    fn zero_stats_render() {
        let view = CardView {
            stats: stats::collect(&[], chrono::Local::now()),
            repo_name: None,
            agents: vec![],
        };
        let png = render_png(&view).unwrap();
        assert_eq!(png_size(&png), (1200, 675));
    }

    #[test]
    fn short_formats() {
        assert_eq!(short(999), "999");
        assert_eq!(short(1_234), "1.2k");
        assert_eq!(short(4_100_000), "4.1M");
        assert_eq!(short(1_200_000_000), "1.2B");
        assert_eq!(short(999_960), "1.0M");
    }

    #[test]
    fn big_numbers_and_many_agents_fit() {
        let mut view = sample();
        view.stats.tokens_saved = 9_999_999_999;
        view.stats.graph.tokens = 9_999_999_999;
        view.agents = [
            "Claude Code",
            "Codex",
            "Cursor",
            "Gemini",
            "Windsurf",
            "Zed",
            "Aider",
        ]
        .map(String::from)
        .to_vec();
        render_png(&view).unwrap();
        let svg = drawn(&view);
        assert!(svg.contains("+4"));
        assert!(svg.contains("Cursor"));
        for name in ["Gemini", "Windsurf", "Zed", "Aider"] {
            assert!(!svg.contains(name), "{name} should be folded into +4");
        }
    }

    #[test]
    fn python_edits_never_drawn() {
        assert!(!drawn(&sample()).to_lowercase().contains("python"));
    }

    #[test]
    fn repo_name_is_escaped() {
        let mut view = sample();
        view.repo_name = Some("a&b<c".into());
        let svg = drawn(&view);
        assert!(svg.contains("a&amp;b&lt;c"));
        render_png(&view).unwrap();
    }

    #[test]
    #[ignore = "writes sample PNGs; run with CARD_SAMPLE_DIR set"]
    fn write_samples() {
        let dir = std::path::PathBuf::from(std::env::var("CARD_SAMPLE_DIR").unwrap());
        let mut view = sample();
        view.repo_name = Some("knot".into());
        view.agents.push("Gemini".into());
        std::fs::write(dir.join("card-sample.png"), render_png(&view).unwrap()).unwrap();
        let zero = CardView {
            stats: stats::collect(&[], chrono::Local::now()),
            repo_name: None,
            agents: vec![],
        };
        std::fs::write(dir.join("card-zero.png"), render_png(&zero).unwrap()).unwrap();
    }

    #[test]
    fn headline_without_reads_shows_a_dash() {
        let mut view = sample();
        assert!(drawn(&view).contains("\u{2212}68%"));
        view.stats.less_to_read = None;
        assert!(drawn(&view).contains("\u{2014}"));
    }
}
