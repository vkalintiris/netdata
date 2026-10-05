//! A badge: the small SVG image of `/api/v1/badge.svg` and `/api/v3/badge.svg`, ported from C's
//! `src/web/api/v1/api_v1_badge/web_buffer_svg.c`. This part is what needs nothing of the agent: the width of a text
//! in the badge's font, the XML escape, the color a value takes from a color expression, a color argument, and the
//! SVG text itself (`buffer_svg()`). The value's own text is [`format_value_and_unit_precision`].
//!
//! Texts are bytes all the way: a label reaches the SVG as the request gave it, cut by bytes where C cuts.

use netdata_agent_text::units::format_value_and_unit_precision;

/// `BADGE_HORIZONTAL_PADDING`.
const HORIZONTAL_PADDING: f64 = 4.0;
/// `VERDANA_KERNING` and `VERDANA_PADDING`.
const KERNING: f64 = 0.2;
const PADDING: f64 = 1.0;
/// `LABEL_STRING_SIZE`, `VALUE_STRING_SIZE` and `COLOR_STRING_SIZE`.
const LABEL_STRING_SIZE: usize = 200;
const VALUE_STRING_SIZE: usize = 100;
const COLOR_STRING_SIZE: usize = 100;
/// `BADGE_SVG_COLOR_ARG_MAXLEN`.
const COLOR_ARG_MAXLEN: usize = 20;
/// `RRDR_OPTION_DISPLAY_ABS`, the one query option the SVG reads (C hands the options over as 32 bits).
const OPTION_DISPLAY_ABS: u32 = netdata_agent_query::tables::options::DISPLAY_ABS as u32;

/// `verdana11_widths[]`: the width of each ASCII character in Verdana at 11 px; 0 for one that is not drawn.
#[rustfmt::skip]
const VERDANA11_WIDTHS: [f64; 128] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 3.8671874999999996, 4.3291015625, 5.048828125, 9.001953125,
    6.9931640625, 11.837890625, 7.992187499999999, 2.9541015625, 4.9951171875, 4.9951171875,
    6.9931640625, 9.001953125, 4.00146484375, 4.9951171875, 4.00146484375, 4.9951171875,
    6.9931640625, 6.9931640625, 6.9931640625, 6.9931640625, 6.9931640625, 6.9931640625,
    6.9931640625, 6.9931640625, 6.9931640625, 6.9931640625, 4.9951171875, 4.9951171875,
    9.001953125, 9.001953125, 9.001953125, 5.99951171875, 11.0, 7.51953125,
    7.541015625, 7.680664062499999, 8.4755859375, 6.95556640625, 6.32177734375, 8.529296875,
    8.26611328125, 4.6298828125, 5.00048828125, 7.62158203125, 6.123046875, 9.2705078125,
    8.228515625, 8.658203125, 6.63330078125, 8.658203125, 7.6484375, 7.51953125,
    6.7783203125, 8.05126953125, 7.51953125, 10.87646484375, 7.53564453125, 6.767578125,
    7.53564453125, 4.9951171875, 4.9951171875, 4.9951171875, 9.001953125, 6.9931640625,
    6.9931640625, 6.6064453125, 6.853515625, 5.73095703125, 6.853515625, 6.552734375,
    3.8671874999999996, 6.853515625, 6.9609375, 3.0185546875, 3.78662109375, 6.509765625,
    3.0185546875, 10.69921875, 6.9609375, 6.67626953125, 6.853515625, 6.853515625,
    4.6943359375, 5.73095703125, 4.33447265625, 6.9609375, 6.509765625, 9.001953125,
    6.509765625, 6.509765625, 5.779296875, 6.982421875, 4.9951171875, 6.982421875,
    9.001953125, 0.0,
];

/// `verdana11_width(s, 11.0)`: the width of a text at the badge's font size. A multi-byte character counts as one em
/// with no kerning; a byte with the high bit that starts none counts nothing; the last kerning is taken back and
/// the padding added, so the empty text is 0.8 wide.
pub fn verdana11_width(text: &[u8]) -> f64 {
    const EM_SIZE: f64 = 11.0;
    // `IS_UTF8_BYTE` and `IS_UTF8_STARTBYTE`
    let is_utf8 = |b: u8| b & 0x80 != 0;
    let is_start = |b: u8| b & 0xc0 == 0xc0;
    let mut w = 0.0;
    let mut i = 0;
    while i < text.len() {
        let b = text[i];
        if is_start(b) {
            i += 1;
            while i < text.len() && is_utf8(text[i]) && !is_start(text[i]) {
                i += 1;
            }
            w += EM_SIZE;
        } else {
            if !is_utf8(b) {
                let t = VERDANA11_WIDTHS[usize::from(b)];
                if t != 0.0 {
                    w += t + KERNING;
                }
            }
            i += 1;
        }
    }
    w -= KERNING;
    w += PADDING;
    w
}

/// `escape_xmlz(dst, src, len)`: the text with `&`, `<`, `>`, `"` and `'` as entities and a backslash as a slash,
/// in at most `len` bytes. An entity that would fill the room to its last byte is not written and the text ends
/// there; a multi-byte character can be cut.
pub fn escape_xmlz(src: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len().min(len));
    let mut room = len;
    for &b in src {
        if room == 0 {
            break;
        }
        let entity: &[u8] = match b {
            b'\\' => {
                out.push(b'/');
                room -= 1;
                continue;
            }
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' => b"&quot;",
            b'\'' => b"&apos;",
            _ => {
                out.push(b);
                room -= 1;
                continue;
            }
        };
        if room <= entity.len() {
            break;
        }
        out.extend_from_slice(entity);
        room -= entity.len();
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

/// `calc_colorz(color, final, len, value)`: the color a value takes from an expression like
/// `red>90|yellow>70|green`, in at most `len` bytes. Entries are tried in order and the first whose test holds, or
/// that has no test, stops the search. C then takes the color text of the entry it stopped at, or of the LAST entry
/// when none held; with an empty color text there, the whole expression. A threshold is an integer (`str2l()`), an
/// empty one or `null` is no value, and a value that is no number takes only such an entry, whatever the operator.
pub fn calc_colorz(color: &[u8], len: usize, value: f64) -> Vec<u8> {
    let value = if value.is_finite() { value } else { f64::NAN };
    let mut color_buffer: Vec<u8> = Vec::new();
    let mut value_buffer: Vec<u8> = Vec::new();
    // the comparison is kept from one entry to the next, as C's variable is
    let mut comparison = Comparison::Greater;

    let mut c = 0;
    while c < color.len() {
        color_buffer.clear();
        let mut tested = false;
        // C counts a threshold's bytes per entry: a second operator starts the text again, not the count
        let (mut ci, mut vi) = (0usize, 0usize);
        let mut t = c;
        while t < color.len() && color[t] != b'|' {
            let next = color.get(t + 1).copied();
            let mut operator = |comparison_now: Comparison, value_buffer: &mut Vec<u8>| {
                comparison = comparison_now;
                value_buffer.clear();
                tested = true;
            };
            match color[t] {
                b'!' => {
                    if next == Some(b'=') {
                        t += 1;
                    }
                    operator(Comparison::NotEqual, &mut value_buffer);
                }
                b'=' | b':' => operator(Comparison::Equal, &mut value_buffer),
                b'}' | b')' | b'>' => {
                    if next == Some(b'=') {
                        t += 1;
                        operator(Comparison::GreaterEqual, &mut value_buffer);
                    } else {
                        operator(Comparison::Greater, &mut value_buffer);
                    }
                }
                b'{' | b'(' | b'<' => {
                    if next == Some(b'=') {
                        t += 1;
                        operator(Comparison::LessEqual, &mut value_buffer);
                    } else if matches!(next, Some(b'>' | b')' | b'}')) {
                        t += 1;
                        operator(Comparison::NotEqual, &mut value_buffer);
                    } else {
                        operator(Comparison::Less, &mut value_buffer);
                    }
                }
                b => {
                    if tested {
                        if vi < 256 {
                            vi += 1;
                            value_buffer.push(b);
                        }
                    } else if ci < 256 {
                        ci += 1;
                        color_buffer.push(b);
                    }
                }
            }
            t += 1;
        }
        if t < color.len() && color[t] == b'|' {
            t += 1;
        }
        c = t;

        if !tested {
            break;
        }
        let threshold = if value_buffer.is_empty() || value_buffer == b"null" {
            f64::NAN
        } else {
            netdata_agent_text::parse::str2l(&value_buffer) as f64
        };
        if value.is_nan() || threshold.is_nan() {
            if value.is_nan() && threshold.is_nan() {
                break;
            }
        } else {
            let holds = match comparison {
                Comparison::Less => value < threshold,
                Comparison::LessEqual => value <= threshold,
                Comparison::Greater => value > threshold,
                Comparison::GreaterEqual => value >= threshold,
                Comparison::Equal => value == threshold,
                Comparison::NotEqual => value != threshold,
            };
            if holds {
                break;
            }
        }
    }

    // C's buffers are texts: a NUL ends one
    let chosen = netdata_agent_text::c::c_str(&color_buffer);
    let mut out = if chosen.is_empty() { netdata_agent_text::c::c_str(color).to_vec() } else { chosen.to_vec() };
    out.truncate(len);
    out
}

/// The badge's named colors (`badge_colors[]`).
const COLORS: [(&[u8], &[u8]); 11] = [
    (b"brightgreen", b"4c1"),
    (b"green", b"97CA00"),
    (b"yellow", b"dfb317"),
    (b"yellowgreen", b"a4a61d"),
    (b"orange", b"fe7d37"),
    (b"red", b"e05d44"),
    (b"blue", b"007ec6"),
    (b"grey", b"555"),
    (b"gray", b"555"),
    (b"lightgrey", b"9f9f9f"),
    (b"lightgray", b"9f9f9f"),
];

/// `parse_color_argument(arg, def)`: a color for the SVG: three or six hex digits as they are, a known name's
/// color, else the default. An argument shorter than 2 bytes or of 20 and more is the default.
pub fn parse_color_argument<'a>(arg: Option<&'a [u8]>, def: &'a [u8]) -> &'a [u8] {
    let Some(arg) = arg else {
        return def;
    };
    let arg = netdata_agent_text::c::c_str(arg);
    if arg.len() < 2 || arg.len() >= COLOR_ARG_MAXLEN {
        return def;
    }
    // html_color_check(): RGB or RRGGBB
    if (arg.len() == 3 || arg.len() == 6) && arg.iter().all(u8::is_ascii_hexdigit) {
        return arg;
    }
    COLORS.iter().find(|(name, _)| *name == arg).map_or(def, |(_, color)| *color)
}

/// What `buffer_svg()` draws.
pub struct Svg<'a> {
    pub label: &'a [u8],
    pub value: f64,
    pub units: &'a [u8],
    pub label_color: Option<&'a [u8]>,
    pub value_color: Option<&'a [u8]>,
    pub precision: i32,
    pub scale: i32,
    pub options: u32,
    pub fixed_width_lbl: i32,
    pub fixed_width_val: i32,
    pub text_color_lbl: Option<&'a [u8]>,
    pub text_color_val: Option<&'a [u8]>,
}

/// C's `%0.2f` and `%0.0f` of a finite or infinite double.
fn fixed(out: &mut Vec<u8>, value: f64, digits: usize) {
    netdata_agent_text::print::print_fixed(out, value, digits);
}

/// `buffer_svg()`: the SVG text of a badge. The widths are the texts' in the badge's font unless both fixed widths
/// are above zero; then the texts are clipped to them and no script is added.
pub fn buffer_svg(svg: &Svg<'_>) -> Vec<u8> {
    let (mut height, mut font_size, mut text_offset, mut round_corner) = (20.0f64, 11.0f64, 5.8f64, 3.0f64);
    let scale = svg.scale.max(100);
    let fixed_widths = svg.fixed_width_lbl > 0 && svg.fixed_width_val > 0;

    let value_color: &[u8] = match svg.value_color.map(netdata_agent_text::c::c_str) {
        Some(color) if !color.is_empty() => color,
        _ if svg.value.is_finite() => b"4c1",
        _ => b"999",
    };
    let value_color_buffer = calc_colorz(value_color, COLOR_STRING_SIZE, svg.value);
    let shown = if svg.options & OPTION_DISPLAY_ABS != 0 { svg.value.abs() } else { svg.value };
    let value_string = format_value_and_unit_precision(shown, svg.units, svg.precision);

    let label = netdata_agent_text::c::c_str(svg.label);
    let (mut label_width, mut value_width) = (f64::from(svg.fixed_width_lbl), f64::from(svg.fixed_width_val));
    if !fixed_widths {
        label_width = verdana11_width(label) + HORIZONTAL_PADDING * 2.0;
        value_width = verdana11_width(&value_string) + HORIZONTAL_PADDING * 2.0;
    }
    let mut total_width = label_width + value_width;

    let label_escaped = escape_xmlz(label, LABEL_STRING_SIZE);
    let value_escaped = escape_xmlz(&value_string, VALUE_STRING_SIZE);
    let label_color = parse_color_argument(svg.label_color, b"555");
    let value_color = parse_color_argument(Some(&value_color_buffer), b"555");

    let scale = f64::from(scale);
    total_width = total_width * scale / 100.0;
    height = height * scale / 100.0;
    font_size = font_size * scale / 100.0;
    text_offset = text_offset * scale / 100.0;
    label_width = label_width * scale / 100.0;
    value_width = value_width * scale / 100.0;
    round_corner = round_corner * scale / 100.0;

    let mut out = Vec::with_capacity(2600);
    let text = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());
    text(&mut out, "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"");
    fixed(&mut out, total_width, 2);
    text(&mut out, "\" height=\"");
    fixed(&mut out, height, 2);
    text(
        &mut out,
        "\"><linearGradient id=\"smooth\" x2=\"0\" y2=\"100%\"><stop offset=\"0\" stop-color=\"#bbb\" \
         stop-opacity=\".1\"/><stop offset=\"1\" stop-opacity=\".1\"/></linearGradient><mask id=\"round\">\
         <rect class=\"bdge-ttl-width\" width=\"",
    );
    fixed(&mut out, total_width, 2);
    text(&mut out, "\" height=\"");
    fixed(&mut out, height, 2);
    text(&mut out, "\" rx=\"");
    fixed(&mut out, round_corner, 2);
    text(&mut out, "\" fill=\"#fff\"/></mask><g mask=\"url(#round)\"><rect class=\"bdge-rect-lbl\" width=\"");
    fixed(&mut out, label_width, 2);
    text(&mut out, "\" height=\"");
    fixed(&mut out, height, 2);
    text(&mut out, "\" fill=\"#");
    out.extend_from_slice(label_color);
    text(&mut out, "\"/>");

    if fixed_widths {
        text(&mut out, "<clipPath id=\"lbl-rect\"><rect class=\"bdge-rect-lbl\" width=\"");
        fixed(&mut out, label_width, 2);
        text(&mut out, "\" height=\"");
        fixed(&mut out, height, 2);
        text(&mut out, "\"/></clipPath>");
    }

    text(&mut out, "<rect class=\"bdge-rect-val\" x=\"");
    fixed(&mut out, label_width, 2);
    text(&mut out, "\" width=\"");
    fixed(&mut out, value_width, 2);
    text(&mut out, "\" height=\"");
    fixed(&mut out, height, 2);
    text(&mut out, "\" fill=\"#");
    out.extend_from_slice(value_color);
    text(&mut out, "\"/>");

    if fixed_widths {
        text(&mut out, "<clipPath id=\"val-rect\"><rect class=\"bdge-rect-val\" x=\"");
        fixed(&mut out, label_width, 2);
        text(&mut out, "\" width=\"");
        fixed(&mut out, value_width, 2);
        text(&mut out, "\" height=\"");
        fixed(&mut out, height, 2);
        text(&mut out, "\"/></clipPath>");
    }

    text(&mut out, "<rect class=\"bdge-ttl-width\" width=\"");
    fixed(&mut out, total_width, 2);
    text(&mut out, "\" height=\"");
    fixed(&mut out, height, 2);
    text(
        &mut out,
        "\" fill=\"url(#smooth)\"/></g><g text-anchor=\"middle\" \
         font-family=\"DejaVu Sans,Verdana,Geneva,sans-serif\" font-size=\"",
    );
    fixed(&mut out, font_size, 2);
    text(&mut out, "\">");

    // the four texts: a shadow and the text itself, for the label and for the value
    let label_x = label_width / 2.0;
    let value_x = label_width + value_width / 2.0 - 1.0;
    let shadow_y = (height - text_offset).ceil();
    let text_y = (height - text_offset - 1.0).ceil();
    let text_color_lbl = parse_color_argument(svg.text_color_lbl, b"fff");
    let text_color_val = parse_color_argument(svg.text_color_val, b"fff");
    struct Line<'a> {
        class: &'a str,
        x: f64,
        y: f64,
        fill: Option<&'a [u8]>,
        clip: &'a str,
        body: &'a [u8],
    }
    let line = |class, x, y, fill, clip, body| Line { class, x, y, fill, clip, body };
    let lines = [
        line("bdge-lbl-lbl", label_x, shadow_y, None, "lbl-rect", &label_escaped),
        line("bdge-lbl-lbl", label_x, text_y, Some(text_color_lbl), "lbl-rect", &label_escaped),
        line("bdge-lbl-val", value_x, shadow_y, None, "val-rect", &value_escaped),
        line("bdge-lbl-val", value_x, text_y, Some(text_color_val), "val-rect", &value_escaped),
    ];
    for Line { class, x, y, fill, clip, body } in lines {
        text(&mut out, "<text class=\"");
        text(&mut out, class);
        text(&mut out, "\" x=\"");
        fixed(&mut out, x, 2);
        text(&mut out, "\" y=\"");
        fixed(&mut out, y, 0);
        match fill {
            None => text(&mut out, "\" fill=\"#010101\" fill-opacity=\".3\""),
            Some(color) => {
                text(&mut out, "\" fill=\"#");
                out.extend_from_slice(color);
                text(&mut out, "\"");
            }
        }
        text(&mut out, " clip-path=\"url(#");
        text(&mut out, clip);
        text(&mut out, ")\">");
        out.extend_from_slice(body);
        text(&mut out, "</text>");
    }
    text(&mut out, "</g>");

    if !fixed_widths {
        text(&mut out, SCRIPT);
    }
    text(&mut out, "</svg>");
    out
}

/// The script a badge without fixed widths carries: it measures the two texts in the browser and sets the widths.
const SCRIPT: &str = "<script type=\"text/javascript\">\
var bdg_horiz_padding = 4;\
function netdata_bdge_each(list, attr, value){\
Array.prototype.forEach.call(list, function(el){\
el.setAttribute(attr, value);\
});\
};\
var this_svg = document.currentScript.closest(\"svg\");\
var elem_lbl = this_svg.getElementsByClassName(\"bdge-lbl-lbl\");\
var elem_val = this_svg.getElementsByClassName(\"bdge-lbl-val\");\
var lbl_size = elem_lbl[0].getBBox();\
var val_size = elem_val[0].getBBox();\
var width_total = lbl_size.width + bdg_horiz_padding*2;\
this_svg.getElementsByClassName(\"bdge-rect-lbl\")[0].setAttribute(\"width\", width_total);\
netdata_bdge_each(elem_lbl, \"x\", (lbl_size.width / 2) + bdg_horiz_padding);\
netdata_bdge_each(elem_val, \"x\", width_total + (val_size.width / 2) + bdg_horiz_padding);\
var val_rect = this_svg.getElementsByClassName(\"bdge-rect-val\")[0];\
val_rect.setAttribute(\"width\", val_size.width + bdg_horiz_padding*2);\
val_rect.setAttribute(\"x\", width_total);\
width_total += val_size.width + bdg_horiz_padding*2;\
var width_update_elems = this_svg.getElementsByClassName(\"bdge-ttl-width\");\
netdata_bdge_each(width_update_elems, \"width\", width_total);\
this_svg.setAttribute(\"width\", width_total);\
</script>";
