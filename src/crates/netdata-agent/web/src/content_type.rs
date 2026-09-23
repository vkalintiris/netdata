//! Content types, ported from `src/libnetdata/http/content_type.c`.

/// `HTTP_CONTENT_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentType {
    ApplicationJson,
    TextPlain,
    TextEventStream,
    TextHtml,
    TextCss,
    TextYaml,
    ApplicationYaml,
    TextXml,
    TextXsl,
    ApplicationXml,
    ApplicationXJavascript,
    ApplicationOctetStream,
    ImageSvgXml,
    ApplicationXFontTruetype,
    ApplicationXFontOpentype,
    ApplicationFontWoff,
    ApplicationFontWoff2,
    ApplicationVndMsFontobj,
    ImagePng,
    ImageJpg,
    ImageGif,
    ImageXicon,
    ImageBmp,
    ImageIcns,
    AudioMpeg,
    AudioOgg,
    VideoMp4,
    ApplicationPdf,
    ApplicationZip,
    ApplicationWasm,
    Prometheus,
}

struct Entry {
    name: &'static str,
    content_type: ContentType,
    needs_charset: bool,
    options: Option<&'static str>,
}

const fn e(name: &'static str, content_type: ContentType, needs_charset: bool) -> Entry {
    Entry {
        name,
        content_type,
        needs_charset,
        options: None,
    }
}

/// `content_types[]` in table order: lookups in both directions take the first match, so the secondary entries
/// only add names.
const TABLE: &[Entry] = &[
    e("application/json", ContentType::ApplicationJson, true),
    e("text/plain", ContentType::TextPlain, true),
    e("text/event-stream", ContentType::TextEventStream, true),
    e("text/html", ContentType::TextHtml, true),
    e("text/css", ContentType::TextCss, true),
    e("text/yaml", ContentType::TextYaml, true),
    e("application/yaml", ContentType::ApplicationYaml, true),
    e("text/xml", ContentType::TextXml, true),
    e("text/xsl", ContentType::TextXsl, true),
    e("application/xml", ContentType::ApplicationXml, true),
    e(
        "application/javascript",
        ContentType::ApplicationXJavascript,
        true,
    ),
    e(
        "application/octet-stream",
        ContentType::ApplicationOctetStream,
        false,
    ),
    e("image/svg+xml", ContentType::ImageSvgXml, false),
    e(
        "application/x-font-truetype",
        ContentType::ApplicationXFontTruetype,
        false,
    ),
    e(
        "application/x-font-opentype",
        ContentType::ApplicationXFontOpentype,
        false,
    ),
    e(
        "application/font-woff",
        ContentType::ApplicationFontWoff,
        false,
    ),
    e(
        "application/font-woff2",
        ContentType::ApplicationFontWoff2,
        false,
    ),
    e(
        "application/vnd.ms-fontobject",
        ContentType::ApplicationVndMsFontobj,
        false,
    ),
    e("image/png", ContentType::ImagePng, false),
    e("image/jpeg", ContentType::ImageJpg, false),
    e("image/gif", ContentType::ImageGif, false),
    e("image/x-icon", ContentType::ImageXicon, false),
    e("image/bmp", ContentType::ImageBmp, false),
    e("image/icns", ContentType::ImageIcns, false),
    e("audio/mpeg", ContentType::AudioMpeg, false),
    e("audio/ogg", ContentType::AudioOgg, false),
    e("video/mp4", ContentType::VideoMp4, false),
    e("application/pdf", ContentType::ApplicationPdf, false),
    e("application/zip", ContentType::ApplicationZip, false),
    e("application/wasm", ContentType::ApplicationWasm, false),
    e("image/png", ContentType::ImagePng, false),
    // secondary - overlapping with primary
    Entry {
        name: "text/plain",
        content_type: ContentType::Prometheus,
        needs_charset: true,
        options: Some("version=0.0.4"),
    },
    e("prometheus", ContentType::Prometheus, true),
    e("text", ContentType::TextPlain, true),
    e("txt", ContentType::TextPlain, true),
    e("json", ContentType::ApplicationJson, true),
    e("html", ContentType::TextHtml, true),
    e("xml", ContentType::ApplicationXml, true),
];

/// `mime_types[]` in `src/libnetdata/http/http_defs.c`: file extensions of the static files the web server sends.
const MIME_TYPES: &[(&str, ContentType)] = &[
    ("html", ContentType::TextHtml),
    ("js", ContentType::ApplicationXJavascript),
    ("css", ContentType::TextCss),
    ("xml", ContentType::TextXml),
    ("xsl", ContentType::TextXsl),
    ("txt", ContentType::TextPlain),
    ("svg", ContentType::ImageSvgXml),
    ("ttf", ContentType::ApplicationXFontTruetype),
    ("otf", ContentType::ApplicationXFontOpentype),
    ("woff2", ContentType::ApplicationFontWoff2),
    ("woff", ContentType::ApplicationFontWoff),
    ("eot", ContentType::ApplicationVndMsFontobj),
    ("png", ContentType::ImagePng),
    ("jpg", ContentType::ImageJpg),
    ("jpeg", ContentType::ImageJpg),
    ("gif", ContentType::ImageGif),
    ("bmp", ContentType::ImageBmp),
    ("ico", ContentType::ImageXicon),
    ("icns", ContentType::ImageIcns),
    ("wasm", ContentType::ApplicationWasm),
];

impl ContentType {
    /// `contenttype_for_filename()`: by the text after the last dot of the whole path, which may lie in a directory
    /// name; no extension or an unknown one is `application/octet-stream`.
    pub fn for_filename(filename: &[u8]) -> Self {
        let Some(dot) = filename.iter().rposition(|&c| c == b'.') else {
            return ContentType::ApplicationOctetStream;
        };
        let extension = &filename[dot + 1..];
        MIME_TYPES
            .iter()
            .find(|(name, _)| !extension.is_empty() && name.as_bytes() == extension)
            .map_or(ContentType::ApplicationOctetStream, |&(_, content_type)| {
                content_type
            })
    }

    /// `content_type_string2id()`: exact, case-sensitive name match; unknown and empty names are `text/plain`.
    pub fn from_name(name: &[u8]) -> Self {
        TABLE
            .iter()
            .find(|entry| entry.name.as_bytes() == name)
            .map_or(ContentType::TextPlain, |entry| entry.content_type)
    }

    /// `content_type_id2string()`: the first name registered for this type.
    pub fn name(self) -> &'static str {
        TABLE
            .iter()
            .find(|entry| entry.content_type == self)
            .map_or("text/plain", |entry| entry.name)
    }

    /// `http_header_content_type()`: the complete `Content-Type:` header line, CRLF included.
    pub fn write_header(self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"Content-Type: ");
        match TABLE.iter().find(|entry| entry.content_type == self) {
            Some(entry) => {
                out.extend_from_slice(entry.name.as_bytes());
                if entry.needs_charset {
                    out.extend_from_slice(b"; charset=utf-8");
                }
                if let Some(options) = entry.options {
                    out.extend_from_slice(b"; ");
                    out.extend_from_slice(options.as_bytes());
                }
                out.extend_from_slice(b"\r\n");
            }
            None => out.extend_from_slice(b"text/plain; charset=utf-8\r\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_for_filename() {
        let cases: [(&[u8], ContentType); 5] = [
            (b"/web/v2/index.html", ContentType::TextHtml),
            (b"/web/app.woff2", ContentType::ApplicationFontWoff2),
            (b"/web/conf.d/README", ContentType::ApplicationOctetStream),
            (b"/web/x.", ContentType::ApplicationOctetStream),
            (b"/web/x.HTML", ContentType::ApplicationOctetStream),
        ];
        for (name, expected) in cases {
            assert_eq!(ContentType::for_filename(name), expected);
        }
    }

    #[test]
    fn names_and_headers() {
        assert_eq!(
            ContentType::from_name(b"text/plain"),
            ContentType::TextPlain
        );
        assert_eq!(
            ContentType::from_name(b"prometheus"),
            ContentType::Prometheus
        );
        assert_eq!(
            ContentType::from_name(b"Application/JSON"),
            ContentType::TextPlain
        );
        assert_eq!(ContentType::from_name(b""), ContentType::TextPlain);

        let header = |ct: ContentType| {
            let mut out = Vec::new();
            ct.write_header(&mut out);
            String::from_utf8(out).unwrap()
        };
        assert_eq!(
            header(ContentType::ApplicationJson),
            "Content-Type: application/json; charset=utf-8\r\n"
        );
        assert_eq!(header(ContentType::ImagePng), "Content-Type: image/png\r\n");
        // Prometheus is first found in the secondary "text/plain" entry, with its options.
        assert_eq!(
            header(ContentType::Prometheus),
            "Content-Type: text/plain; charset=utf-8; version=0.0.4\r\n"
        );
    }
}
