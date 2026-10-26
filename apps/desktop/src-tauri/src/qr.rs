//! Draws the pairing QR code.
//!
//! The protocol asks for byte mode and error correction level M, so the
//! segments are chosen here instead of letting the encoder pick (it would
//! switch to alphanumeric mode for text that happens to be upper case).
//!
//! The image is drawn for phone cameras: one path with a rectangle per run of
//! dark modules (so neighbours touch with no seams), a 4-module quiet zone,
//! and a size that is a whole number of pixels per module.

use std::fmt::Write as _;

use qrcode::bits::Bits;
use qrcode::{Color, EcLevel, QrCode, Version};

/// Quiet zone around the code, in modules (the QR standard asks for 4).
const QUIET_ZONE: usize = 4;
/// The image is at least this many pixels wide, at a whole pixel per module.
const MIN_IMAGE_PX: usize = 240;

/// The payload does not fit in any QR code at level M.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the pairing payload is too long for a QR code")]
pub struct TooLong;

/// Renders `payload` as an SVG document: black modules on a white background
/// with a quiet zone, so it scans the same in light and dark themes.
///
/// The same payload always gives the same document.
pub fn svg(payload: &str) -> Result<String, TooLong> {
    let code = encode(payload.as_bytes())?;
    let width = code.width();
    let colors = code.to_colors();
    let dark = |x: usize, y: usize| colors[y * width + x] == Color::Dark;

    // One subpath per horizontal run of dark modules. Writing to a String
    // cannot fail, so the results are ignored.
    let mut path = String::new();
    for y in 0..width {
        let mut x = 0;
        while x < width {
            if !dark(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && dark(x, y) {
                x += 1;
            }
            let run = x - start;
            let _ = write!(
                path,
                "M{} {}h{run}v1h-{run}z",
                start + QUIET_ZONE,
                y + QUIET_ZONE
            );
        }
    }

    let modules = width + 2 * QUIET_ZONE;
    let px = modules * MIN_IMAGE_PX.div_ceil(modules);
    Ok(format!(
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {m} {m}" "##,
            r##"width="{px}" height="{px}" shape-rendering="crispEdges">"##,
            r##"<rect width="{m}" height="{m}" fill="#ffffff"/>"##,
            r##"<path fill="#000000" d="{path}"/></svg>"##
        ),
        m = modules,
        px = px,
        path = path
    ))
}

/// Smallest version that holds `data` in byte mode at level M.
fn encode(data: &[u8]) -> Result<QrCode, TooLong> {
    for version in 1..=40 {
        let mut bits = Bits::new(Version::Normal(version));
        if bits.push_byte_data(data).is_err() || bits.push_terminator(EcLevel::M).is_err() {
            continue;
        }
        return QrCode::with_bits(bits, EcLevel::M).map_err(|_| TooLong);
    }
    Err(TooLong)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "link2linux://pair?v=1&f=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&s=BBBBBBBBBBBBBBBBBBBBBB&n=Desk&a=192.168.1.20";

    /// The module matrix an SVG from [`svg`] describes, quiet zone included.
    struct Parsed {
        modules: usize,
        px: usize,
        dark: Vec<bool>,
    }

    fn attr<'a>(svg: &'a str, name: &str) -> &'a str {
        let key = format!("{name}=\"");
        let start = svg.find(&key).unwrap() + key.len();
        &svg[start..start + svg[start..].find('"').unwrap()]
    }

    /// Reads the SVG back the way a renderer would, so the test checks the
    /// document that is shown and not just the encoder behind it.
    fn parse(svg: &str) -> Parsed {
        let view: Vec<usize> = attr(svg, "viewBox")
            .split(' ')
            .map(|n| n.parse().unwrap())
            .collect();
        let modules = view[2];
        assert_eq!(view, [0, 0, modules, modules]);
        let px = attr(svg, "width").parse().unwrap();
        assert_eq!(attr(svg, "height").parse::<usize>().unwrap(), px);

        let mut dark = vec![false; modules * modules];
        for part in attr(svg, "d").split('z').filter(|p| !p.is_empty()) {
            // M<x> <y>h<run>v1h-<run>
            let rest = part.strip_prefix('M').unwrap();
            let (origin, rest) = rest.split_once('h').unwrap();
            let (x, y) = origin.split_once(' ').unwrap();
            let (run, tail) = rest.split_once('v').unwrap();
            let (x, y, run): (usize, usize, usize) =
                (x.parse().unwrap(), y.parse().unwrap(), run.parse().unwrap());
            assert_eq!(tail, format!("1h-{run}"));
            for dx in 0..run {
                dark[y * modules + x + dx] = true;
            }
        }
        Parsed { modules, px, dark }
    }

    /// Decodes the parsed image with an independent QR reader.
    fn decode(parsed: &Parsed, scale: usize) -> String {
        let size = parsed.modules * scale;
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(size, size, |x, y| {
            if parsed.dark[(y / scale) * parsed.modules + x / scale] {
                0
            } else {
                255
            }
        });
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1, "exactly one code is found");
        let (_, content) = grids[0].decode().expect("the code decodes");
        content
    }

    #[test]
    fn draws_an_svg_document() {
        let svg = svg(URI).unwrap();
        assert!(svg.starts_with("<svg"), "{}", &svg[..40]);
        assert!(svg.contains("#000000") && svg.contains("#ffffff"));
        assert!(svg.contains("shape-rendering=\"crispEdges\""));
    }

    #[test]
    fn same_payload_gives_same_image() {
        assert_eq!(svg(URI), svg(URI));
        assert_ne!(svg(URI), svg("link2linux://pair?v=1&n=Other"));
    }

    #[test]
    fn decodes_back_to_the_same_uri() {
        let payloads = [
            URI,
            "link2linux://pair?v=1&n=Desk",
            "x",
            "link2linux://pair?v=1&n=Caf%C3%A9%20Desk&a=[fd00::2]:27381,10.0.0.2",
            &"a".repeat(400),
        ];
        for payload in payloads {
            let parsed = parse(&svg(payload).unwrap());
            for scale in [4, 7] {
                assert_eq!(decode(&parsed, scale), payload, "scale {scale}");
            }
        }
    }

    #[test]
    fn keeps_a_four_module_quiet_zone() {
        let parsed = parse(&svg(URI).unwrap());
        let n = parsed.modules;
        for y in 0..n {
            for x in 0..n {
                if x < 4 || y < 4 || x >= n - 4 || y >= n - 4 {
                    assert!(
                        !parsed.dark[y * n + x],
                        "dark module in quiet zone at {x},{y}"
                    );
                }
            }
        }
        // The finder pattern's corner module sits right at the quiet zone.
        assert!(parsed.dark[4 * n + 4]);
    }

    #[test]
    fn uses_a_whole_number_of_pixels_per_module() {
        for payload in ["x", URI, &"a".repeat(400)] {
            let parsed = parse(&svg(payload).unwrap());
            assert_eq!(parsed.px % parsed.modules, 0, "{payload:.10}");
            assert!(parsed.px >= MIN_IMAGE_PX);
            assert!(parsed.px / parsed.modules >= 1);
        }
    }

    #[test]
    fn draws_runs_not_single_squares() {
        let svg = svg(URI).unwrap();
        assert!(!svg.contains("<rect x="), "no per-module rectangles");
        // A 7-module finder row is a single run.
        assert!(svg.contains("h7v1h-7z"));
    }

    #[test]
    fn uses_byte_mode_at_level_m() {
        // 16 upper-case characters fit version 1 in alphanumeric mode but need
        // version 2 as bytes (14 bytes fit version 1 at level M).
        let code = encode(b"LINK2LINUX 12345").unwrap();
        assert_eq!(code.error_correction_level(), EcLevel::M);
        assert_eq!(code.version(), Version::Normal(2));
    }

    #[test]
    fn typical_payload_stays_small() {
        let code = encode(URI.as_bytes()).unwrap();
        assert!(matches!(code.version(), Version::Normal(v) if v <= 8));
    }

    #[test]
    fn rejects_payloads_that_cannot_fit() {
        let huge = "x".repeat(3000);
        assert_eq!(svg(&huge), Err(TooLong));
    }
}
