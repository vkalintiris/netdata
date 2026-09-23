//! The machine GUID, ported from `src/daemon/machine-guid.c`: read from `<lib>/registry/netdata.public.unique.id`,
//! or generated and saved there. (The status-file fallback and the blacklist come with the status file.)

use std::io::Write;
use std::path::Path;

/// `machine_guid_get()`.
pub fn machine_guid_get(varlib: &str) -> std::io::Result<String> {
    let dir = Path::new(varlib).join("registry");
    let file = dir.join("netdata.public.unique.id");
    if let Ok(text) = std::fs::read_to_string(&file) {
        let text = text.trim();
        if uuid::Uuid::parse_str(text).is_ok() && text.len() == 36 {
            return Ok(text.to_ascii_lowercase());
        }
    }
    let guid = uuid::Uuid::new_v4().hyphenated().to_string();
    std::fs::create_dir_all(&dir)?;
    // Written as a temporary file and renamed into place, 36 bytes without a newline, like C.
    let tmp = dir.join(format!("netdata.public.unique.id.{}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(guid.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &file)?;
    Ok(guid)
}
