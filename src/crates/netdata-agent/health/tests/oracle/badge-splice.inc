// SPDX-License-Identifier: GPL-3.0-or-later
//
// Appended to a copy of src/web/api/v1/api_v1_badge/web_buffer_svg.c, so that the oracle can call the file's static
// functions. Nothing of C's text is changed: the copy is the file followed by these lines.

double oracle_verdana11_width(const char *s) {
    // buffer_svg() measures at its font size, 11.0, before the scale
    return verdana11_width(s, 11.0);
}

size_t oracle_escape_xmlz(char *dst, const char *src, size_t len) {
    return escape_xmlz(dst, src, len);
}

void oracle_calc_colorz(const char *color, char *final, size_t len, NETDATA_DOUBLE value) {
    calc_colorz(color, final, len, value);
}

const char *oracle_parse_color_argument(const char *arg, const char *def) {
    return parse_color_argument(arg, def);
}

void oracle_buffer_svg(BUFFER *wb, const char *label, NETDATA_DOUBLE value, const char *units, const char *label_color,
                       const char *value_color, int precision, int scale, uint32_t options, int fixed_width_lbl,
                       int fixed_width_val, const char *text_color_lbl, const char *text_color_val) {
    buffer_svg(wb, label, value, units, label_color, value_color, precision, scale, options, fixed_width_lbl,
               fixed_width_val, text_color_lbl, text_color_val);
}
