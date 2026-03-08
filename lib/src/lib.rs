/// Approximate [Bézier curves](https://en.wikipedia.org/wiki/B%C3%A9zier_curve) with [Circular arcs](https://en.wikipedia.org/wiki/Circular_arc)
mod arc;
/// Converts an SVG to an internal representation
mod converter;
/// Emulates the state of an arbitrary machine that can run G-Code
mod machine;
/// Operations that are easier to implement while/after G-Code is generated, or would
/// otherwise over-complicate SVG conversion
mod postprocess;
/// Provides an interface for drawing lines in G-Code
/// This concept is referred to as [Turtle graphics](https://en.wikipedia.org/wiki/Turtle_graphics).
mod turtle;

pub use converter::{
    ConversionConfig, ConversionOptions, LayerMode, LayerOverrideOptions, SvgLayerInfo,
    extract_svg_layers, svg_layer_key, svg2program,
};
pub use machine::{Machine, MachineConfig, SupportedFunctionality};
pub use postprocess::PostprocessConfig;
pub use turtle::Turtle;

/// A cross-platform type used to store all configuration types.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Settings {
    pub conversion: ConversionConfig,
    pub machine: MachineConfig,
    pub postprocess: PostprocessConfig,
    #[cfg_attr(feature = "serde", serde(default = "Version::unknown"))]
    pub version: Version,
}

impl Settings {
    /// Try to automatically upgrade the supported version.
    ///
    /// This will return an error if:
    ///
    /// - Settings version is [`Version::Unknown`].
    /// - There are breaking changes requiring manual intervention. In which case this does a partial update to that point.
    pub fn try_upgrade(&mut self) -> Result<(), &'static str> {
        loop {
            match self.version {
                // Compatibility for M2 by default
                Version::V0 => {
                    self.machine.end_sequence = Some(format!(
                        "{} M2",
                        self.machine.end_sequence.take().unwrap_or_default()
                    ));
                    self.version = Version::V5;
                }
                Version::V5 => break Ok(()),
                Version::Unknown(_) => break Err("cannot upgrade unknown version"),
            }
        }
    }
}

/// Used to control breaking change behavior for [`Settings`].
///
/// There were already 3 non-breaking version bumps (V1 -> V4) so versioning starts off with [`Version::V5`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Version {
    /// Implicitly versioned settings from before this type was introduced.
    V0,
    /// M2 is no longer appended to the program by default
    V5,
    #[cfg_attr(feature = "serde", serde(untagged))]
    Unknown(String),
}

impl Version {
    /// Returns the most recent [`Version`]. This is useful for asking users to upgrade externally-stored settings.
    pub const fn latest() -> Self {
        Self::V5
    }

    /// Default version for old settings.
    pub const fn unknown() -> Self {
        Self::V0
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Version::V0 => f.write_str("V0"),
            Version::V5 => f.write_str("V5"),
            Version::Unknown(unknown) => f.write_str(unknown),
        }
    }
}

impl Default for Version {
    fn default() -> Self {
        Self::latest()
    }
}

#[cfg(test)]
mod test {
    use g_code::emit::{FormatOptions, Token};
    use pretty_assertions::assert_eq;
    use roxmltree::ParsingOptions;
    use svgtypes::{Length, LengthUnit};

    use super::*;

    /// The values change between debug and release builds for circular interpolation,
    /// so only check within a rough tolerance
    const TOLERANCE: f64 = 1E-10;

    fn get_actual(
        input: &str,
        circular_interpolation: bool,
        dimensions: [Option<Length>; 2],
    ) -> Vec<Token<'_>> {
        let config = ConversionConfig::default();
        let options = ConversionOptions {
            dimensions,
            ..Default::default()
        };
        let document = roxmltree::Document::parse_with_options(
            input,
            ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();

        let machine = Machine::new(
            SupportedFunctionality {
                circular_interpolation,
            },
            None,
            None,
            None,
            None,
        );
        converter::svg2program(&document, &config, options, machine)
    }

    fn assert_close(left: Vec<Token<'_>>, right: Vec<Token<'_>>) {
        let mut code = String::new();
        g_code::emit::format_gcode_fmt(left.iter(), FormatOptions::default(), &mut code).unwrap();
        assert_eq!(left.len(), right.len(), "{code}");
        for (i, pair) in left.into_iter().zip(right.into_iter()).enumerate() {
            match pair {
                (Token::Field(l), Token::Field(r)) => {
                    assert_eq!(l.letters, r.letters);
                    if let (Some(l_value), Some(r_value)) = (l.value.as_f64(), r.value.as_f64()) {
                        assert!(
                            (l_value - r_value).abs() < TOLERANCE,
                            "Values differ significantly at {i}: {l} vs {r} ({})",
                            (l_value - r_value).abs()
                        );
                    } else {
                        assert_eq!(l, r);
                    }
                }
                (l, r) => {
                    assert_eq!(l, r, "Differs at {i}");
                }
            }
        }
    }

    #[test]
    fn square_produces_expected_gcode() {
        let expected = g_code::parse::file_parser(include_str!("../tests/square.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(include_str!("../tests/square.svg"), false, [None; 2]);

        assert_close(actual, expected);
    }

    #[test]
    fn square_dimension_override_produces_expected_gcode() {
        let side_length = Length {
            number: 10.,
            unit: LengthUnit::Mm,
        };

        let expected = g_code::parse::file_parser(include_str!("../tests/square.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();

        for square in [
            include_str!("../tests/square.svg"),
            include_str!("../tests/square_dimensionless.svg"),
        ] {
            assert_close(
                get_actual(square, false, [Some(side_length); 2]),
                expected.clone(),
            );
            assert_close(
                get_actual(square, false, [Some(side_length), None]),
                expected.clone(),
            );
            assert_close(
                get_actual(square, false, [None, Some(side_length)]),
                expected.clone(),
            );
        }
    }

    #[test]
    fn square_transformed_produces_expected_gcode() {
        let square_transformed = include_str!("../tests/square_transformed.svg");
        let expected =
            g_code::parse::file_parser(include_str!("../tests/square_transformed.gcode"))
                .unwrap()
                .iter_emit_tokens()
                .collect::<Vec<_>>();
        let actual = get_actual(square_transformed, false, [None; 2]);

        assert_close(actual, expected)
    }

    #[test]
    fn square_transformed_nested_produces_expected_gcode() {
        let square_transformed = include_str!("../tests/square_transformed_nested.svg");
        let expected =
            g_code::parse::file_parser(include_str!("../tests/square_transformed_nested.gcode"))
                .unwrap()
                .iter_emit_tokens()
                .collect::<Vec<_>>();
        let actual = get_actual(square_transformed, false, [None; 2]);

        assert_close(actual, expected)
    }

    #[test]
    fn square_viewport_produces_expected_gcode() {
        let square_viewport = include_str!("../tests/square_viewport.svg");
        let expected = g_code::parse::file_parser(include_str!("../tests/square_viewport.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(square_viewport, false, [None; 2]);

        assert_close(actual, expected);
    }

    #[test]
    fn circular_interpolation_produces_expected_gcode() {
        let circular_interpolation = include_str!("../tests/circular_interpolation.svg");
        let expected =
            g_code::parse::file_parser(include_str!("../tests/circular_interpolation.gcode"))
                .unwrap()
                .iter_emit_tokens()
                .collect::<Vec<_>>();
        let actual = get_actual(circular_interpolation, true, [None; 2]);

        assert_close(actual, expected)
    }

    #[test]
    fn svg_with_smooth_curves_produces_expected_gcode() {
        let svg = include_str!("../tests/smooth_curves.svg");

        let expected = g_code::parse::file_parser(include_str!("../tests/smooth_curves.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();

        let file = if cfg!(debug) {
            include_str!("../tests/smooth_curves_circular_interpolation.gcode")
        } else {
            include_str!("../tests/smooth_curves_circular_interpolation_release.gcode")
        };
        let expected_circular_interpolation = g_code::parse::file_parser(file)
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        assert_close(get_actual(svg, false, [None; 2]), expected);

        assert_close(
            get_actual(svg, true, [None; 2]),
            expected_circular_interpolation,
        );
    }

    #[test]
    fn shapes_produces_expected_gcode() {
        let shapes = include_str!("../tests/shapes.svg");
        let expected = g_code::parse::file_parser(include_str!("../tests/shapes.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(shapes, false, [None; 2]);

        assert_close(actual, expected)
    }

    #[test]
    fn use_defs_produces_expected_gcode() {
        let svg = include_str!("../tests/use_defs.svg");
        let expected = g_code::parse::file_parser(include_str!("../tests/use_defs.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(svg, false, [None; 2]);

        assert_close(actual, expected)
    }

    #[test]
    fn use_xlink_href_produces_expected_gcode() {
        let svg = include_str!("../tests/use_xlink_href.svg");
        let expected = g_code::parse::file_parser(include_str!("../tests/use_xlink_href.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(svg, false, [None; 2]);

        assert_close(actual, expected)
    }

    #[test]
    fn use_symbol_produces_expected_gcode() {
        let svg = include_str!("../tests/use_symbol.svg");
        let expected = g_code::parse::file_parser(include_str!("../tests/use_symbol.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(svg, false, [None; 2]);

        assert_close(actual, expected);
    }

    #[test]
    fn transform_origin_produces_expected_gcode() {
        let svg = include_str!("../tests/transform_origin.svg");
        let expected = g_code::parse::file_parser(include_str!("../tests/transform_origin.gcode"))
            .unwrap()
            .iter_emit_tokens()
            .collect::<Vec<_>>();
        let actual = get_actual(svg, false, [None; 2]);
        assert_close(actual, expected)
    }

    /// `transform-origin="5 5"` with `rotate(90)` should be identical to the
    /// manual SVG equivalent `translate(5,5) rotate(90) translate(-5,-5)`
    #[test]
    fn layer_feedrate_override_produces_correct_feedrate() {
        let svg = include_str!("../tests/layer_settings.svg");
        let config = ConversionConfig::default(); // global feedrate = 300
        let options = ConversionOptions::default();
        let document = roxmltree::Document::parse_with_options(
            svg,
            ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();
        let machine = Machine::new(
            SupportedFunctionality {
                circular_interpolation: false,
            },
            None,
            None,
            None,
            None,
        );
        let tokens = converter::svg2program(&document, &config, options, machine);

        let mut gcode = String::new();
        g_code::emit::format_gcode_fmt(
            tokens.iter(),
            g_code::emit::FormatOptions::default(),
            &mut gcode,
        )
        .unwrap();

        // Layer 1 overrides feedrate to 600 — all G1 moves in that layer must use F600
        // Layer 2 overrides feedrate to 150 — all G1 moves in that layer must use F150
        // The global feedrate (300) should never appear since both layers override it
        assert!(
            gcode.contains("F600"),
            "Expected F600 from layer 1 feedrate override, got:\n{gcode}"
        );
        assert!(
            gcode.contains("F150"),
            "Expected F150 from layer 2 feedrate override, got:\n{gcode}"
        );
        assert!(
            !gcode.contains("F300"),
            "Global feedrate F300 should not appear when all layers override it, got:\n{gcode}"
        );
    }

    #[test]
    fn layer_power_override_emits_spindle_command() {
        let svg = include_str!("../tests/layer_settings.svg");
        let config = ConversionConfig::default();
        let options = ConversionOptions::default();
        let document = roxmltree::Document::parse_with_options(
            svg,
            ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();
        let machine = Machine::new(
            SupportedFunctionality {
                circular_interpolation: false,
            },
            None,
            None,
            None,
            None,
        );
        let tokens = converter::svg2program(&document, &config, options, machine);

        let mut gcode = String::new();
        g_code::emit::format_gcode_fmt(
            tokens.iter(),
            g_code::emit::FormatOptions::default(),
            &mut gcode,
        )
        .unwrap();

        // Power is emitted as S inline on G1 commands (GRBL laser mode style).
        // No M3/M5 per-path toggling — G0 automatically disables the laser in GRBL ($32=1).
        assert!(
            !gcode.contains("M3"),
            "M3 should not appear per-path; power is set via S word inline on G1, got:\n{gcode}"
        );
        assert!(
            gcode.contains("S80"),
            "Expected S80 inline on G1 from layer 1 power override, got:\n{gcode}"
        );
        assert!(
            gcode.contains("S255"),
            "Expected S255 inline on G1 from layer 2 power override, got:\n{gcode}"
        );
    }

    #[test]
    fn layer_passes_repeats_path_correct_number_of_times() {
        let svg = include_str!("../tests/layer_settings.svg");
        let config = ConversionConfig::default();
        let options = ConversionOptions::default();
        let document = roxmltree::Document::parse_with_options(
            svg,
            ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();
        let machine = Machine::new(
            SupportedFunctionality {
                circular_interpolation: false,
            },
            None,
            None,
            None,
            None,
        );
        let tokens = converter::svg2program(&document, &config, options, machine);

        let mut gcode = String::new();
        g_code::emit::format_gcode_fmt(
            tokens.iter(),
            g_code::emit::FormatOptions::default(),
            &mut gcode,
        )
        .unwrap();

        // Layer 1 has data-passes="2": the square path has 4 sides, so G1 at F600 should
        // appear 4 * 2 = 8 times. Count occurrences of "F600" as a proxy for G1 moves in
        // that layer.
        let f600_count = gcode.matches("F600").count();
        assert_eq!(
            f600_count, 8,
            "Layer 1 (2 passes × 4 sides = 8 G1 moves at F600), got {f600_count}:\n{gcode}"
        );

        // Layer 2 has data-passes="3": 4 sides × 3 = 12 G1 moves at F150.
        let f150_count = gcode.matches("F150").count();
        assert_eq!(
            f150_count, 12,
            "Layer 2 (3 passes × 4 sides = 12 G1 moves at F150), got {f150_count}:\n{gcode}"
        );
    }

    #[test]
    fn transform_origin_matches_manual_equivalent() {
        let with_origin = get_actual(
            include_str!("../tests/transform_origin.svg"),
            false,
            [None; 2],
        );
        let manual = get_actual(
            include_str!("../tests/transform_origin_equivalent.svg"),
            false,
            [None; 2],
        );
        assert_close(with_origin, manual)
    }

    #[test]
    #[cfg(feature = "serde")]
    fn deserialize_v1_config_succeeds() {
        let json = r#"
        {
            "conversion": {
              "tolerance": 0.002,
              "feedrate": 300.0,
              "dpi": 96.0
            },
            "machine": {
              "supported_functionality": {
                "circular_interpolation": true
              },
              "tool_on_sequence": null,
              "tool_off_sequence": null,
              "begin_sequence": null,
              "end_sequence": null
            },
            "postprocess": {
              "origin": [
                0.0,
                0.0
              ]
            }
          }
        "#;
        serde_json::from_str::<Settings>(json).unwrap();
    }

    #[test]
    #[cfg(feature = "serde")]
    fn deserialize_v2_config_succeeds() {
        let json = r#"
        {
            "conversion": {
              "tolerance": 0.002,
              "feedrate": 300.0,
              "dpi": 96.0
            },
            "machine": {
              "supported_functionality": {
                "circular_interpolation": true
              },
              "tool_on_sequence": null,
              "tool_off_sequence": null,
              "begin_sequence": null,
              "end_sequence": null
            },
            "postprocess": { }
          }
        "#;
        serde_json::from_str::<Settings>(json).unwrap();
    }

    #[test]
    #[cfg(feature = "serde")]
    fn deserialize_v3_config_succeeds() {
        let json = r#"
        {
            "conversion": {
              "tolerance": 0.002,
              "feedrate": 300.0,
              "dpi": 96.0
            },
            "machine": {
              "supported_functionality": {
                "circular_interpolation": true
              },
              "tool_on_sequence": null,
              "tool_off_sequence": null,
              "begin_sequence": null,
              "end_sequence": null
            },
            "postprocess": {
                "checksums": false,
                "line_numbers": false
            }
          }
        "#;
        serde_json::from_str::<Settings>(json).unwrap();
    }

    #[test]
    #[cfg(feature = "serde")]
    fn deserialize_v4_config_succeeds() {
        let json = r#"
        {
            "conversion": {
              "tolerance": 0.002,
              "feedrate": 300.0,
              "dpi": 96.0
            },
            "machine": {
              "supported_functionality": {
                "circular_interpolation": true
              },
              "tool_on_sequence": null,
              "tool_off_sequence": null,
              "begin_sequence": null,
              "end_sequence": null
            },
            "postprocess": {
                "checksums": false,
                "line_numbers": false,
                "newline_before_comment": false
            }
          }
        "#;
        serde_json::from_str::<Settings>(json).unwrap();
    }

    #[test]
    #[cfg(feature = "serde")]
    fn deserialize_v5_config_succeeds() {
        let json = r#"
        {
            "conversion": {
              "tolerance": 0.002,
              "feedrate": 300.0,
              "dpi": 96.0
            },
            "machine": {
              "supported_functionality": {
                "circular_interpolation": true
              },
              "tool_on_sequence": null,
              "tool_off_sequence": null,
              "begin_sequence": null,
              "end_sequence": null
            },
            "postprocess": {
                "checksums": false,
                "line_numbers": false,
                "newline_before_comment": false
            },
            "version": "V5"
          }
        "#;
        serde_json::from_str::<Settings>(json).unwrap();
    }

    /// Verify that fill-mode hatch lines land *within* the shape's bounding box
    /// and not outside it (e.g. due to a double Y-flip bug).
    ///
    /// The SVG contains a 10×10 mm square in a layer.  With Fill mode and a
    /// 0.5 mm beam width we expect horizontal hatch lines whose Y coordinates
    /// are all between 0 and 10 mm (the square's extent after origin mapping).
    #[test]
    fn fill_mode_hatch_lines_are_within_shape_bounds() {
        // Minimal SVG: 10×10 mm square inside a single <g> layer.
        let svg = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg"
     width="10mm" height="10mm" viewBox="0 0 10 10">
  <g id="layer1">
    <rect x="0" y="0" width="10" height="10"/>
  </g>
</svg>"#;

        let config = ConversionConfig {
            beam_width: 0.5,
            ..ConversionConfig::default()
        };

        let mut layer_overrides = std::collections::HashMap::new();
        layer_overrides.insert(
            "layer1".to_owned(),
            LayerOverrideOptions {
                mode: Some(LayerMode::Fill),
                ..Default::default()
            },
        );
        let options = ConversionOptions {
            layer_overrides,
            ..Default::default()
        };

        let document = roxmltree::Document::parse_with_options(
            svg,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();

        let machine = Machine::new(
            SupportedFunctionality {
                circular_interpolation: false,
            },
            None,
            None,
            None,
            None,
        );

        let tokens = converter::svg2program(&document, &config, options, machine);

        let mut gcode = String::new();
        g_code::emit::format_gcode_fmt(
            tokens.iter(),
            g_code::emit::FormatOptions::default(),
            &mut gcode,
        )
        .unwrap();

        // Extract all Y values from G0/G1 lines.
        let y_values: Vec<f64> = gcode
            .lines()
            .filter_map(|line| {
                // Match lines like "G0 X... Y..." or "G1 X... Y... F..."
                let y_pos = line.find('Y')?;
                let rest = &line[y_pos + 1..];
                let end = rest
                    .find(|c: char| c == ' ' || c == '\n')
                    .unwrap_or(rest.len());
                rest[..end].parse::<f64>().ok()
            })
            .collect();

        assert!(
            !y_values.is_empty(),
            "No Y coordinates found in GCode output:\n{gcode}"
        );

        // The square spans 0–10 mm in Y.  All Y coordinates (outline + hatch)
        // must be within a small epsilon of that range.
        let eps = 1e-3;
        for y in &y_values {
            assert!(
                *y >= -eps && *y <= 10.0 + eps,
                "Y coordinate {y} is outside [0, 10] mm — possible Y-flip bug.\nGCode:\n{gcode}"
            );
        }

        // There must be hatch fill lines (multiple Y values, not just the 4 corners).
        assert!(
            y_values.len() > 4,
            "Expected hatch fill lines but only found {} Y coordinates.\nGCode:\n{gcode}",
            y_values.len()
        );
    }

    /// Verify that Fill mode on a circle layer produces hatch lines whose X
    /// extents are clipped to the circle's interior — not the bounding rectangle.
    ///
    /// A circle with centre (5, 5) mm and radius 5 mm is placed in a 10×10 mm
    /// SVG viewport.  With Fill mode the scanline algorithm must intersect the
    /// circle outline, so every hatch segment endpoint must satisfy
    ///   (x - 5)² + (y - 5)² ≤ (5 + ε)²
    ///
    /// If the old bounding-box approach were still used the hatch lines would
    /// span the full 0–10 mm in X regardless of Y, meaning the corners
    /// (x=0, y=5) and (x=10, y=5) etc. would wrongly be included.
    #[test]
    fn fill_mode_circle_hatch_stays_within_circle() {
        // Circle: centre (5,5) mm, radius 5 mm — exactly fills the viewport.
        let svg = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg"
     width="10mm" height="10mm" viewBox="0 0 10 10">
  <g id="circle_layer">
    <circle cx="5" cy="5" r="5"/>
  </g>
</svg>"#;

        let config = ConversionConfig {
            // 0.5 mm spacing gives enough lines to verify clipping without being
            // so dense that the test is slow.
            beam_width: 0.5,
            // Use a fine tolerance so the arc is flattened accurately.
            tolerance: 0.01,
            ..ConversionConfig::default()
        };

        let mut layer_overrides = std::collections::HashMap::new();
        layer_overrides.insert(
            "circle_layer".to_owned(),
            LayerOverrideOptions {
                mode: Some(LayerMode::Fill),
                ..Default::default()
            },
        );
        let options = ConversionOptions {
            layer_overrides,
            ..Default::default()
        };

        let document = roxmltree::Document::parse_with_options(
            svg,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();

        let machine = Machine::new(
            SupportedFunctionality {
                circular_interpolation: false,
            },
            None,
            None,
            None,
            None,
        );

        let tokens = converter::svg2program(&document, &config, options, machine);

        let mut gcode = String::new();
        g_code::emit::format_gcode_fmt(
            tokens.iter(),
            g_code::emit::FormatOptions::default(),
            &mut gcode,
        )
        .unwrap();

        // ── helpers ────────────────────────────────────────────────────────────

        /// Extract all (X, Y) coordinate pairs from G0/G1 lines in GCode output.
        fn extract_xy(gcode: &str) -> Vec<(f64, f64)> {
            gcode
                .lines()
                .filter_map(|line| {
                    let x_pos = line.find('X')?;
                    let y_pos = line.find('Y')?;
                    let x_rest = &line[x_pos + 1..];
                    let y_rest = &line[y_pos + 1..];
                    let x_end = x_rest
                        .find(|c: char| c == ' ' || c == '\n')
                        .unwrap_or(x_rest.len());
                    let y_end = y_rest
                        .find(|c: char| c == ' ' || c == '\n')
                        .unwrap_or(y_rest.len());
                    let x = x_rest[..x_end].parse::<f64>().ok()?;
                    let y = y_rest[..y_end].parse::<f64>().ok()?;
                    Some((x, y))
                })
                .collect()
        }

        let coords = extract_xy(&gcode);
        assert!(
            !coords.is_empty(),
            "No XY coordinates found in GCode output:\n{gcode}"
        );

        // The circle is at centre (5,5) with radius 5 mm.
        let cx = 5.0_f64;
        let cy = 5.0_f64;
        let r = 5.0_f64;

        // Allow a small epsilon for flattening / rounding error.
        // The tolerance is 0.01 mm so 0.05 mm is generous.
        let eps = 0.05_f64;

        for &(x, y) in &coords {
            let dist = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            assert!(
                dist <= r + eps,
                "Hatch point ({x:.4}, {y:.4}) lies outside the circle \
                 (distance from centre = {dist:.4} mm, radius = {r} mm).\n\
                 This means the fill is hatching the bounding box, not the circle.\n\
                 GCode:\n{gcode}"
            );
        }

        // Additionally verify that hatch lines were actually produced and are not
        // trivially short: we should see many G1 (draw) moves.
        let g1_count = gcode.lines().filter(|l| l.starts_with("G1")).count();
        assert!(
            g1_count >= 10,
            "Expected at least 10 G1 hatch moves for a filled circle, got {g1_count}.\n\
             GCode:\n{gcode}"
        );

        // Sanity: the bounding-box approach would produce hatch lines reaching
        // X=0 and X=10 at the equator (y≈5).  Check that no such full-width
        // lines exist (i.e. that clipping is actually happening).
        let equator_x_values: Vec<f64> = coords
            .iter()
            .filter(|&&(_, y)| (y - cy).abs() < 0.6)
            .map(|&(x, _)| x)
            .collect();
        assert!(
            !equator_x_values.is_empty(),
            "No hatch points found near equator (y≈{cy}) — circle may not have been filled."
        );
        // At the equator the chord runs from x=0 to x=10.  With clipping the
        // endpoints must be close to those values (within the flattening epsilon).
        let min_x_equator = equator_x_values
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max_x_equator = equator_x_values
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        // The chord at y=5 should start near x=0 and end near x=10.
        assert!(
            min_x_equator < 0.5,
            "Equator hatch starts too far right (min_x={min_x_equator:.4}); \
             fill may not reach the left edge of the circle."
        );
        assert!(
            max_x_equator > 9.5,
            "Equator hatch ends too far left (max_x={max_x_equator:.4}); \
             fill may not reach the right edge of the circle."
        );
    }
}
