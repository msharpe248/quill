//! A message's own colours, for a page shown dark.
//!
//! Mail is designed for a white page, and says so: a white table, dark
//! text. Shown dark it would be a bright page in a dark window. So where
//! the page is to be dark, each colour the message names is given another
//! lightness by what it is the colour of: a light background becomes a
//! dark one, dark text becomes light, and a border is drawn somewhere in
//! between. The hue stays, so a red is still a red; a background that is
//! already dark and text that is already light are left exactly as they
//! were, so mail designed dark is not turned round. Pictures are not
//! touched at all.
//!
//! This reads CSS the sanitizer has already judged (no comments, no
//! escapes, every `url()` quoted) and changes nothing in it but a colour
//! for a colour and what `color-scheme` says. What it writes in a colour's
//! place is always `#rrggbb` or `rgba(r,g,b,a)` in digits, so there is
//! nothing it can add that the judging would have refused. What it cannot
//! read as a colour it leaves as it found it.

use std::borrow::Cow;

/// What a colour is the colour of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Background,
    Text,
    Border,
}

/// The lightness a white background is given: what a web view's own dark
/// page has, so a message that paints itself white and one that paints
/// nothing look the same.
const DARKEST: f64 = 0.118;

/// A background darker than this is left as it is.
const DARK_ENOUGH: f64 = 0.4;

/// A colour: red, green and blue from 0 to 255, and how opaque from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Colour {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

/// The value of a `bgcolor` or `color` attribute for a dark page, or
/// `None` where it is not a colour this can read, and is left alone.
pub(crate) fn attribute(value: &str, role: Role) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let colour = match value.strip_prefix('#') {
        Some(digits) if matches!(digits.len(), 3 | 6) => hex(digits)?,
        Some(_) => return None,
        None if value.len() == 6 && named(&value).is_none() => hex(&value)?,
        None => named(&value)?,
    };
    Some(written(shade(colour, role)))
}

/// CSS for a dark page: a whole style sheet, or (`block` false) the
/// declarations of a `style` attribute.
pub(crate) fn css(text: &str, block: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    // How many blocks deep: declarations are only ever inside one.
    let mut depth = usize::from(!block);
    let mut brackets = 0_usize;
    let mut quote: Option<u8> = None;
    let mut start = 0;
    for (at, byte) in bytes.iter().copied().enumerate() {
        if let Some(open) = quote {
            if byte == open || byte == b'\n' {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'(' => brackets += 1,
            b')' => brackets = brackets.saturating_sub(1),
            // A selector, or the start of an at-rule: not a declaration.
            b'{' if brackets == 0 => {
                out.push_str(&text[start..=at]);
                depth += 1;
                start = at + 1;
            }
            b';' | b'}' if brackets == 0 => {
                let piece = &text[start..at];
                if depth > 0 {
                    out.push_str(&declaration(piece));
                } else {
                    out.push_str(piece);
                }
                out.push(char::from(byte));
                if byte == b'}' {
                    depth = depth.saturating_sub(1);
                }
                start = at + 1;
            }
            _ => {}
        }
    }
    let rest = &text[start..];
    if depth > 0 && brackets == 0 && quote.is_none() {
        out.push_str(&declaration(rest));
    } else {
        out.push_str(rest);
    }
    out
}

/// What a property's value holds that a dark page changes.
enum Holds {
    Colours(Role),
    /// `color-scheme`: the page is dark, whatever the message says of it.
    Scheme,
    Nothing,
}

fn holds(property: &str) -> Holds {
    match property {
        "color"
        | "caret-color"
        | "text-decoration"
        | "text-decoration-color"
        | "-webkit-text-fill-color" => Holds::Colours(Role::Text),
        "background" | "background-color" | "background-image" => Holds::Colours(Role::Background),
        "border" | "border-color" | "outline" | "outline-color" | "column-rule"
        | "column-rule-color" => Holds::Colours(Role::Border),
        "color-scheme" => Holds::Scheme,
        // One side of a border, or its colour.
        _ => match property.strip_prefix("border-") {
            Some(side) => {
                let side = side.strip_suffix("-color").unwrap_or(side);
                let sides = [
                    "top",
                    "right",
                    "bottom",
                    "left",
                    "block",
                    "inline",
                    "block-start",
                    "block-end",
                    "inline-start",
                    "inline-end",
                ];
                if sides.contains(&side) {
                    Holds::Colours(Role::Border)
                } else {
                    Holds::Nothing
                }
            }
            None => Holds::Nothing,
        },
    }
}

/// One declaration, `property: value`, or whatever else stood where one
/// could: that is left as it was.
fn declaration(piece: &str) -> Cow<'_, str> {
    let Some((property, value)) = piece.split_once(':') else {
        return Cow::Borrowed(piece);
    };
    let name = property.trim().to_ascii_lowercase();
    let is_name = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if !is_name {
        return Cow::Borrowed(piece);
    }
    match holds(&name) {
        Holds::Colours(role) => Cow::Owned(format!("{property}:{}", colours(value, role))),
        Holds::Scheme => {
            let important = if value.to_ascii_lowercase().contains("!important") {
                " !important"
            } else {
                ""
            };
            Cow::Owned(format!("{property}:dark{important}"))
        }
        Holds::Nothing => Cow::Borrowed(piece),
    }
}

/// A value with each colour in it given its lightness for a dark page.
fn colours(value: &str, role: Role) -> String {
    let bytes = value.as_bytes();
    let word_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
    let run = |from: usize, of: &dyn Fn(u8) -> bool| {
        bytes[from..].iter().take_while(|byte| of(**byte)).count()
    };
    let mut out = String::with_capacity(value.len() + 8);
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        match byte {
            // A string: an address, a font's name. Never a colour.
            b'"' | b'\'' => {
                let end = value[at + 1..]
                    .find(char::from(byte))
                    .map_or(value.len(), |close| at + 1 + close + 1);
                out.push_str(&value[at..end]);
                at = end;
            }
            b'#' => {
                let digits = run(at + 1, &|byte| byte.is_ascii_hexdigit());
                let end = at + 1 + digits;
                let ends_there = bytes.get(end).is_none_or(|next| !word_byte(*next));
                match hex(&value[at + 1..end]).filter(|_| ends_there) {
                    Some(colour) => out.push_str(&written(shade(colour, role))),
                    None => out.push_str(&value[at..end]),
                }
                at = end;
            }
            _ if byte.is_ascii_alphabetic() || byte == b'-' || byte == b'_' => {
                let end = at + run(at, &word_byte);
                let word = value[at..end].to_ascii_lowercase();
                if bytes.get(end) == Some(&b'(') {
                    // A colour written as a function, or some other
                    // function, whose inside is read on from here.
                    let read = value[end + 1..].find(')').and_then(|close| {
                        let colour = function(&word, &value[end + 1..end + 1 + close])?;
                        Some((colour, end + 1 + close + 1))
                    });
                    if let Some((colour, after)) = read {
                        out.push_str(&written(shade(colour, role)));
                        at = after;
                        continue;
                    }
                    out.push_str(&value[at..end]);
                } else if let Some(colour) = named(&word) {
                    out.push_str(&written(shade(colour, role)));
                } else {
                    out.push_str(&value[at..end]);
                }
                at = end;
            }
            // A number, with its unit.
            _ if byte.is_ascii_digit() || byte == b'.' => {
                let end = at
                    + run(at, &|byte| {
                        byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'%'
                    });
                out.push_str(&value[at..end]);
                at = end;
            }
            _ => {
                let length = value[at..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&value[at..at + length]);
                at += length;
            }
        }
    }
    out
}

/// The colour for a dark page of one that was chosen for a light one.
fn shade(colour: Colour, role: Role) -> Colour {
    let (hue, saturation, lightness) = to_hsl(colour);
    let (saturation, lightness) = match role {
        // The lighter it was, the darker it is: white comes out as the
        // dark page itself. A pale tint is all colour once it is that
        // dark, so some of the colour goes with the light.
        Role::Background if lightness > DARK_ENOUGH => {
            let light = (lightness - DARK_ENOUGH) / (1.0 - DARK_ENOUGH);
            (
                saturation * (1.0 - 0.45 * light),
                DARK_ENOUGH - light * (DARK_ENOUGH - DARKEST),
            )
        }
        // As light as it was dark: black comes out white.
        Role::Text if lightness < 0.5 => (saturation, 1.0 - lightness),
        // Seen against a dark page as it was against a light one: faint
        // where it was faint, plain where it was plain.
        Role::Border => (saturation, 0.5 - 0.3 * lightness),
        Role::Background | Role::Text => return colour,
    };
    from_hsl(hue, saturation, lightness, colour.alpha)
}

/// Hue in degrees, saturation and lightness from 0 to 1.
fn to_hsl(colour: Colour) -> (f64, f64, f64) {
    let (red, green, blue) = (
        colour.red / 255.0,
        colour.green / 255.0,
        colour.blue / 255.0,
    );
    let most = red.max(green).max(blue);
    let least = red.min(green).min(blue);
    let lightness = f64::midpoint(most, least);
    let spread = most - least;
    if spread <= f64::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = spread / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if (most - red).abs() <= f64::EPSILON {
        ((green - blue) / spread).rem_euclid(6.0)
    } else if (most - green).abs() <= f64::EPSILON {
        (blue - red) / spread + 2.0
    } else {
        (red - green) / spread + 4.0
    };
    (hue * 60.0, saturation.clamp(0.0, 1.0), lightness)
}

fn from_hsl(hue: f64, saturation: f64, lightness: f64, alpha: f64) -> Colour {
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue.rem_euclid(360.0) / 60.0;
    let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = if sector < 1.0 {
        (chroma, second, 0.0)
    } else if sector < 2.0 {
        (second, chroma, 0.0)
    } else if sector < 3.0 {
        (0.0, chroma, second)
    } else if sector < 4.0 {
        (0.0, second, chroma)
    } else if sector < 5.0 {
        (second, 0.0, chroma)
    } else {
        (chroma, 0.0, second)
    };
    let lift = lightness - chroma / 2.0;
    Colour {
        red: (red + lift) * 255.0,
        green: (green + lift) * 255.0,
        blue: (blue + lift) * 255.0,
        alpha,
    }
}

/// A colour as it is written into the page: digits and nothing else.
fn written(colour: Colour) -> String {
    // Held to a byte's range first, so the cast loses nothing.
    let byte = |part: f64| part.round().clamp(0.0, 255.0) as u8;
    let (red, green, blue) = (byte(colour.red), byte(colour.green), byte(colour.blue));
    if colour.alpha >= 0.999 || colour.alpha.is_nan() {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        let alpha = colour.alpha.clamp(0.0, 1.0);
        format!("rgba({red},{green},{blue},{alpha:.3})")
    }
}

/// A colour in hex digits, without the `#`: three, four, six or eight.
fn hex(digits: &str) -> Option<Colour> {
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |at: usize| {
        char::from(digits.as_bytes()[at])
            .to_digit(16)
            .map(f64::from)
    };
    let short = |at: usize| Some(digit(at)? * 17.0);
    let long = |at: usize| Some(digit(at)? * 16.0 + digit(at + 1)?);
    let (red, green, blue, alpha) = match digits.len() {
        3 => (short(0)?, short(1)?, short(2)?, 255.0),
        4 => (short(0)?, short(1)?, short(2)?, short(3)?),
        6 => (long(0)?, long(2)?, long(4)?, 255.0),
        8 => (long(0)?, long(2)?, long(4)?, long(6)?),
        _ => return None,
    };
    Some(Colour {
        red,
        green,
        blue,
        alpha: alpha / 255.0,
    })
}

/// A colour written as `rgb()`, `rgba()`, `hsl()` or `hsla()`: `inside` is
/// what the brackets hold. `None` for any other function, and for one of
/// these with anything in it but plain numbers.
fn function(name: &str, inside: &str) -> Option<Colour> {
    if !matches!(name, "rgb" | "rgba" | "hsl" | "hsla") {
        return None;
    }
    let parts: Vec<&str> = inside
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if !matches!(parts.len(), 3 | 4) {
        return None;
    }
    let number = |text: &str| {
        let plain = !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'+'));
        text.parse::<f64>()
            .ok()
            .filter(|number| plain && number.is_finite())
    };
    // A share of `whole`: a percentage of it, or a number as it stands.
    let share = |text: &str, whole: f64| match text.strip_suffix('%') {
        Some(percent) => Some(number(percent)? / 100.0 * whole),
        None => number(text),
    };
    let alpha = match parts.get(3) {
        Some(alpha) => share(alpha, 1.0)?.clamp(0.0, 1.0),
        None => 1.0,
    };
    if name.starts_with("rgb") {
        let part = |at: usize| Some(share(parts[at], 255.0)?.clamp(0.0, 255.0));
        return Some(Colour {
            red: part(0)?,
            green: part(1)?,
            blue: part(2)?,
            alpha,
        });
    }
    let hue = number(parts[0].strip_suffix("deg").unwrap_or(parts[0]))?;
    let percent = |at: usize| {
        let text = parts[at];
        Some((number(text.strip_suffix('%').unwrap_or(text))? / 100.0).clamp(0.0, 1.0))
    };
    Some(from_hsl(hue, percent(1)?, percent(2)?, alpha))
}

/// A colour by its name in CSS, in lower case.
fn named(name: &str) -> Option<Colour> {
    let at = NAMED
        .binary_search_by(|(known, _)| (*known).cmp(name))
        .ok()?;
    let rgb = NAMED[at].1;
    let part = |shift: u32| f64::from((rgb >> shift) & 0xff);
    Some(Colour {
        red: part(16),
        green: part(8),
        blue: part(0),
        alpha: 1.0,
    })
}

/// Every colour CSS has a name for, in the order of the names.
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0x00f0_f8ff),
    ("antiquewhite", 0x00fa_ebd7),
    ("aqua", 0x0000_ffff),
    ("aquamarine", 0x007f_ffd4),
    ("azure", 0x00f0_ffff),
    ("beige", 0x00f5_f5dc),
    ("bisque", 0x00ff_e4c4),
    ("black", 0x0000_0000),
    ("blanchedalmond", 0x00ff_ebcd),
    ("blue", 0x0000_00ff),
    ("blueviolet", 0x008a_2be2),
    ("brown", 0x00a5_2a2a),
    ("burlywood", 0x00de_b887),
    ("cadetblue", 0x005f_9ea0),
    ("chartreuse", 0x007f_ff00),
    ("chocolate", 0x00d2_691e),
    ("coral", 0x00ff_7f50),
    ("cornflowerblue", 0x0064_95ed),
    ("cornsilk", 0x00ff_f8dc),
    ("crimson", 0x00dc_143c),
    ("cyan", 0x0000_ffff),
    ("darkblue", 0x0000_008b),
    ("darkcyan", 0x0000_8b8b),
    ("darkgoldenrod", 0x00b8_860b),
    ("darkgray", 0x00a9_a9a9),
    ("darkgreen", 0x0000_6400),
    ("darkgrey", 0x00a9_a9a9),
    ("darkkhaki", 0x00bd_b76b),
    ("darkmagenta", 0x008b_008b),
    ("darkolivegreen", 0x0055_6b2f),
    ("darkorange", 0x00ff_8c00),
    ("darkorchid", 0x0099_32cc),
    ("darkred", 0x008b_0000),
    ("darksalmon", 0x00e9_967a),
    ("darkseagreen", 0x008f_bc8f),
    ("darkslateblue", 0x0048_3d8b),
    ("darkslategray", 0x002f_4f4f),
    ("darkslategrey", 0x002f_4f4f),
    ("darkturquoise", 0x0000_ced1),
    ("darkviolet", 0x0094_00d3),
    ("deeppink", 0x00ff_1493),
    ("deepskyblue", 0x0000_bfff),
    ("dimgray", 0x0069_6969),
    ("dimgrey", 0x0069_6969),
    ("dodgerblue", 0x001e_90ff),
    ("firebrick", 0x00b2_2222),
    ("floralwhite", 0x00ff_faf0),
    ("forestgreen", 0x0022_8b22),
    ("fuchsia", 0x00ff_00ff),
    ("gainsboro", 0x00dc_dcdc),
    ("ghostwhite", 0x00f8_f8ff),
    ("gold", 0x00ff_d700),
    ("goldenrod", 0x00da_a520),
    ("gray", 0x0080_8080),
    ("green", 0x0000_8000),
    ("greenyellow", 0x00ad_ff2f),
    ("grey", 0x0080_8080),
    ("honeydew", 0x00f0_fff0),
    ("hotpink", 0x00ff_69b4),
    ("indianred", 0x00cd_5c5c),
    ("indigo", 0x004b_0082),
    ("ivory", 0x00ff_fff0),
    ("khaki", 0x00f0_e68c),
    ("lavender", 0x00e6_e6fa),
    ("lavenderblush", 0x00ff_f0f5),
    ("lawngreen", 0x007c_fc00),
    ("lemonchiffon", 0x00ff_facd),
    ("lightblue", 0x00ad_d8e6),
    ("lightcoral", 0x00f0_8080),
    ("lightcyan", 0x00e0_ffff),
    ("lightgoldenrodyellow", 0x00fa_fad2),
    ("lightgray", 0x00d3_d3d3),
    ("lightgreen", 0x0090_ee90),
    ("lightgrey", 0x00d3_d3d3),
    ("lightpink", 0x00ff_b6c1),
    ("lightsalmon", 0x00ff_a07a),
    ("lightseagreen", 0x0020_b2aa),
    ("lightskyblue", 0x0087_cefa),
    ("lightslategray", 0x0077_8899),
    ("lightslategrey", 0x0077_8899),
    ("lightsteelblue", 0x00b0_c4de),
    ("lightyellow", 0x00ff_ffe0),
    ("lime", 0x0000_ff00),
    ("limegreen", 0x0032_cd32),
    ("linen", 0x00fa_f0e6),
    ("magenta", 0x00ff_00ff),
    ("maroon", 0x0080_0000),
    ("mediumaquamarine", 0x0066_cdaa),
    ("mediumblue", 0x0000_00cd),
    ("mediumorchid", 0x00ba_55d3),
    ("mediumpurple", 0x0093_70db),
    ("mediumseagreen", 0x003c_b371),
    ("mediumslateblue", 0x007b_68ee),
    ("mediumspringgreen", 0x0000_fa9a),
    ("mediumturquoise", 0x0048_d1cc),
    ("mediumvioletred", 0x00c7_1585),
    ("midnightblue", 0x0019_1970),
    ("mintcream", 0x00f5_fffa),
    ("mistyrose", 0x00ff_e4e1),
    ("moccasin", 0x00ff_e4b5),
    ("navajowhite", 0x00ff_dead),
    ("navy", 0x0000_0080),
    ("oldlace", 0x00fd_f5e6),
    ("olive", 0x0080_8000),
    ("olivedrab", 0x006b_8e23),
    ("orange", 0x00ff_a500),
    ("orangered", 0x00ff_4500),
    ("orchid", 0x00da_70d6),
    ("palegoldenrod", 0x00ee_e8aa),
    ("palegreen", 0x0098_fb98),
    ("paleturquoise", 0x00af_eeee),
    ("palevioletred", 0x00db_7093),
    ("papayawhip", 0x00ff_efd5),
    ("peachpuff", 0x00ff_dab9),
    ("peru", 0x00cd_853f),
    ("pink", 0x00ff_c0cb),
    ("plum", 0x00dd_a0dd),
    ("powderblue", 0x00b0_e0e6),
    ("purple", 0x0080_0080),
    ("rebeccapurple", 0x0066_3399),
    ("red", 0x00ff_0000),
    ("rosybrown", 0x00bc_8f8f),
    ("royalblue", 0x0041_69e1),
    ("saddlebrown", 0x008b_4513),
    ("salmon", 0x00fa_8072),
    ("sandybrown", 0x00f4_a460),
    ("seagreen", 0x002e_8b57),
    ("seashell", 0x00ff_f5ee),
    ("sienna", 0x00a0_522d),
    ("silver", 0x00c0_c0c0),
    ("skyblue", 0x0087_ceeb),
    ("slateblue", 0x006a_5acd),
    ("slategray", 0x0070_8090),
    ("slategrey", 0x0070_8090),
    ("snow", 0x00ff_fafa),
    ("springgreen", 0x0000_ff7f),
    ("steelblue", 0x0046_82b4),
    ("tan", 0x00d2_b48c),
    ("teal", 0x0000_8080),
    ("thistle", 0x00d8_bfd8),
    ("tomato", 0x00ff_6347),
    ("turquoise", 0x0040_e0d0),
    ("violet", 0x00ee_82ee),
    ("wheat", 0x00f5_deb3),
    ("white", 0x00ff_ffff),
    ("whitesmoke", 0x00f5_f5f5),
    ("yellow", 0x00ff_ff00),
    ("yellowgreen", 0x009a_cd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn background(value: &str) -> String {
        colours(value, Role::Background)
    }

    fn text(value: &str) -> String {
        colours(value, Role::Text)
    }

    /// How light a colour this wrote is, from 0 to 1.
    fn lightness(written: &str) -> f64 {
        let colour = hex(written.strip_prefix('#').expect("a hex colour")).expect("a colour");
        to_hsl(colour).2
    }

    #[test]
    fn the_names_are_in_order_and_all_there() {
        assert_eq!(NAMED.len(), 148);
        assert!(NAMED.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(
            named("rebeccapurple").map(written).as_deref(),
            Some("#663399")
        );
        assert_eq!(named("white").map(written).as_deref(), Some("#ffffff"));
        assert_eq!(named("transparent"), None);
        assert_eq!(named("inherit"), None);
    }

    #[test]
    fn a_white_background_is_the_dark_page_and_black_text_is_white() {
        assert_eq!(background("#ffffff"), "#1e1e1e");
        assert_eq!(background("#FFF"), "#1e1e1e");
        assert_eq!(background("white"), "#1e1e1e");
        assert_eq!(background("rgb(255, 255, 255)"), "#1e1e1e");
        assert_eq!(background("rgb(100% 100% 100%)"), "#1e1e1e");
        assert_eq!(background("hsl(0, 0%, 100%)"), "#1e1e1e");
        assert_eq!(text("#000000"), "#ffffff");
        assert_eq!(text("black"), "#ffffff");
        assert_eq!(text("#333"), "#cccccc");
    }

    #[test]
    fn what_is_already_dark_or_already_light_is_left_exactly_as_it_was() {
        // Mail designed dark is not turned round.
        for dark in ["#000000", "#1a1a2e", "#0b5cad", "navy"] {
            let colour = if dark.starts_with('#') {
                dark.to_owned()
            } else {
                "#000080".to_owned()
            };
            assert_eq!(background(dark), colour, "{dark}");
        }
        for light in ["#ffffff", "#e0e0e0", "#ffcc00"] {
            assert_eq!(text(light), light, "{light}");
        }
    }

    #[test]
    fn the_hue_is_kept() {
        // A pale yellow notice is a dark yellow one, and its brown text a
        // light yellow.
        let hue = |written: &str| to_hsl(hex(&written[1..]).expect("a colour")).0;
        let was = hue("#fff3cd");
        let notice = background("#fff3cd");
        assert!((hue(&notice) - was).abs() < 3.0, "{notice}");
        assert!(lightness(&notice) < 0.25, "{notice}");
        let words = text("#664d03");
        assert!((hue(&words) - hue("#664d03")).abs() < 3.0, "{words}");
        assert!(lightness(&words) > 0.75, "{words}");
    }

    #[test]
    fn lighter_backgrounds_come_out_darker_all_the_way() {
        let mut last = 1.0;
        for grey in [0x70_u32, 0x90, 0xb0, 0xd0, 0xf0, 0xff] {
            let now = lightness(&background(&format!("#{grey:02x}{grey:02x}{grey:02x}")));
            assert!(now < last, "{grey:x}: {now} after {last}");
            last = now;
        }
    }

    #[test]
    fn how_opaque_a_colour_is_stays() {
        assert_eq!(background("rgba(255,255,255,0.5)"), "rgba(30,30,30,0.500)");
        assert_eq!(background("rgb(255 255 255 / 50%)"), "rgba(30,30,30,0.500)");
        assert_eq!(background("#ffffff80"), "rgba(30,30,30,0.502)");
        assert_eq!(text("rgba(0, 0, 0, .87)"), "rgba(255,255,255,0.870)");
    }

    #[test]
    fn a_colour_is_found_among_the_rest_of_a_value() {
        assert_eq!(
            background("#fff url(\"./picture-1.png\") no-repeat top left"),
            "#1e1e1e url(\"./picture-1.png\") no-repeat top left"
        );
        assert_eq!(
            background("linear-gradient(to right, #ffffff 0%, white 100%)"),
            "linear-gradient(to right, #1e1e1e 0%, #1e1e1e 100%)"
        );
        assert_eq!(
            colours("1px solid #dddddd", Role::Border),
            format!("1px solid {}", colours("#dddddd", Role::Border))
        );
        assert_eq!(text(" black !important"), " #ffffff !important");
    }

    #[test]
    fn what_is_not_a_colour_is_left_as_it_was() {
        for value in [
            "transparent",
            "inherit",
            "currentColor",
            "none",
            "var(--brand)",
            "rgb(var(--r), 0, 0)",
            "rgb(calc(1 + 2), 0, 0)",
            "rgb(1, 2)",
            "rgb(1e400, 0, 0)",
            "#ggg",
            "#fffff",
            "#fff0f0f0f",
            "#header",
            "url(\"./white.png\")",
            "\"white\"",
            "'#fff'",
            "-webkit-canvas(white-ish)",
            "whiteish",
            "white-space",
            "rgb(255, 255, 255",
            "é #",
            "",
        ] {
            assert_eq!(background(value), value, "{value:?}");
        }
    }

    #[test]
    fn each_property_is_shaded_as_what_it_colours() {
        assert_eq!(
            css(
                "color:#000;background-color:#fff;border:1px solid #000",
                false
            ),
            "color:#ffffff;background-color:#1e1e1e;border:1px solid #808080"
        );
        assert_eq!(
            css("COLOR : black; Background: WHITE", false),
            "COLOR : #ffffff; Background: #1e1e1e"
        );
        // Not colours of anything this knows: left alone, names and all.
        for untouched in [
            "font-family: white, black",
            "box-shadow: 0 0 2px #000",
            "width: 100%",
            "border-collapse: collapse",
            "border-radius: 3px",
            "--brand: #fff",
            "content: \"color: black\"",
            "black",
            ": black",
        ] {
            assert_eq!(css(untouched, false), untouched, "{untouched}");
        }
        assert_eq!(
            css("border-top-color: #000; border-left: 1px solid #000", false),
            "border-top-color: #808080; border-left: 1px solid #808080"
        );
    }

    #[test]
    fn a_sheet_is_changed_only_in_its_declarations() {
        let sheet = "\
            #fff, .white, a:hover, td[bgcolor=white] { color: #000; background: #fff }\n\
            @media (max-width: 600px) and (prefers-color-scheme: dark) {\n\
              .card, #bad { background-color: white; }\n\
              a:link{color:navy}\n\
            }\n\
            @font-face { font-family: \"black; color: black\"; }\n\
            color: black;\n";
        assert_eq!(
            css(sheet, true),
            "\
            #fff, .white, a:hover, td[bgcolor=white] { color: #ffffff; background: #1e1e1e }\n\
            @media (max-width: 600px) and (prefers-color-scheme: dark) {\n\
              .card, #bad { background-color: #1e1e1e; }\n\
              a:link{color:#7f7fff}\n\
            }\n\
            @font-face { font-family: \"black; color: black\"; }\n\
            color: black;\n"
        );
    }

    #[test]
    fn the_page_is_dark_whatever_the_message_says_its_scheme_is() {
        assert_eq!(
            css(":root { color-scheme: light only; }", true),
            ":root { color-scheme:dark; }"
        );
        assert_eq!(
            css("color-scheme: light !important", false),
            "color-scheme:dark !important"
        );
    }

    #[test]
    fn css_that_never_closes_is_passed_through() {
        for odd in [
            "p { color: black",
            "p { background: rgb(255, 255",
            "p { font-family: \"x; color: black }",
            "}}} p { color: black }",
            "{{{{",
            "p { color: black } }",
        ] {
            // Nothing is lost, and nothing is added but colours.
            let dark = css(odd, true);
            assert!(dark.len() >= odd.len(), "{odd:?} became {dark:?}");
        }
        assert_eq!(css("p { color: black", true), "p { color: #ffffff");
        assert_eq!(
            css("p { background: rgb(255, 255", true),
            "p { background: rgb(255, 255"
        );
    }

    #[test]
    fn an_attribute_is_read_as_old_html_wrote_colours() {
        assert_eq!(
            attribute("#FFFFFF", Role::Background).as_deref(),
            Some("#1e1e1e")
        );
        assert_eq!(
            attribute("ffffff", Role::Background).as_deref(),
            Some("#1e1e1e")
        );
        assert_eq!(
            attribute(" White ", Role::Background).as_deref(),
            Some("#1e1e1e")
        );
        assert_eq!(
            attribute("#fff", Role::Background).as_deref(),
            Some("#1e1e1e")
        );
        assert_eq!(attribute("#000000", Role::Text).as_deref(), Some("#ffffff"));
        // Six letters that are also a name are the name.
        assert_eq!(
            attribute("bisque", Role::Text),
            attribute("#ffe4c4", Role::Text)
        );
        for unread in [
            "",
            "transparent",
            "#ffff",
            "rgb(255,255,255)",
            "fffffg",
            "#",
        ] {
            assert_eq!(attribute(unread, Role::Background), None, "{unread:?}");
        }
    }

    #[test]
    fn nothing_is_written_in_a_colours_place_but_digits() {
        // Whatever is read, what replaces it cannot open a tag, name an
        // address or end the style sheet.
        let safe = |written: &str| {
            written.bytes().all(|byte| {
                byte.is_ascii_hexdigit()
                    || matches!(byte, b'#' | b'r' | b'g' | b'(' | b')' | b',' | b'.')
            })
        };
        for (_, rgb) in NAMED {
            for role in [Role::Background, Role::Text, Role::Border] {
                let part = |shift: u32| f64::from((rgb >> shift) & 0xff);
                for alpha in [0.0, 0.25, 1.0] {
                    let colour = Colour {
                        red: part(16),
                        green: part(8),
                        blue: part(0),
                        alpha,
                    };
                    let written = written(shade(colour, role));
                    assert!(safe(&written), "{written}");
                }
            }
        }
        let odd = Colour {
            red: f64::NAN,
            green: f64::INFINITY,
            blue: -4.0,
            alpha: f64::NAN,
        };
        assert!(safe(&written(odd)), "{}", written(odd));
    }
}
