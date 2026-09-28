//! `status-file-dmi.c` on Linux: the machine's DMI texts from sysfs, cleaned as C cleans them. The serials and asset
//! tags C reads are never written to the file (its writer and parser lines are commented out), so they are not read
//! here. The host prefix is not set yet when C reads these (the status file loads before `[global]`), so the paths
//! are always the host's own.

use std::path::Path;

use super::{Dmi, FixedStr};
use netdata_agent_text::c::{c_str, eq_ignore_case};

/// `dmi_clean_field_placeholder()`'s texts: a whole value equal to one, in any case, is no value.
const PLACEHOLDERS: [&str; 38] = [
    "$(DEFAULT_STRING)",
    "Chassis Manufacture",
    "Chassis Manufacturer",
    "Chassis Version",
    "Default string",
    "N/A",
    "NA",
    "NOT SPECIFIED",
    "No Enclosure",
    "None Provided",
    "None",
    "OEM Chassis Manufacturer",
    "OEM Default string000",
    "OEM",
    "OEM_MB",
    "SYSTEM_MANUFACTURER",
    "SmbiosType1_SystemManufacturer",
    "SmbiosType2_BoardManufacturer",
    "Standard",
    "System Product Name",
    "System UUID",
    "System Version",
    "System manufacturer",
    "TBD by OEM",
    "TBD",
    "To be filled by O.E.M.",
    "Type2 - Board Manufacturer",
    "Type2 - Board Vendor Name1",
    "Unknow",
    "Unknown",
    "XXXXX",
    "default",
    "empty",
    "unspecified",
    "x.x",
    "(null)",
    "0123456789",
    "SKU",
];

/// `dmi_clean_field()`: non-ASCII and control bytes become spaces; a text without a letter or digit is empty;
/// whitespace is trimmed and collapsed (`trim_all()`); a placeholder is empty.
pub(super) fn clean_field(raw: &[u8]) -> Vec<u8> {
    let text: Vec<u8> = c_str(raw)
        .iter()
        .map(|&c| if !c.is_ascii() || c.is_ascii_control() { b' ' } else { c })
        .collect();
    if !text.iter().any(u8::is_ascii_alphanumeric) {
        return Vec::new();
    }
    let words: Vec<&[u8]> = text.split(|&c| c == b' ').filter(|w| !w.is_empty()).collect();
    let text = words.join(&b' ');
    if PLACEHOLDERS.iter().any(|p| eq_ignore_case(&text, p.as_bytes())) {
        return Vec::new();
    }
    text
}

/// `access(R_OK)`, as C checks each candidate path.
fn readable(path: &Path) -> bool {
    nix::unistd::access(path, nix::unistd::AccessFlags::R_OK).is_ok()
}

/// `linux_get_dmi_field()` under `root` (`/` outside tests): the class path, else the virtual one, else `alt`; one
/// read of at most 255 bytes, cleaned, then cut to the field's size (a cut may leave a trailing space, as in C).
fn read_field<const N: usize>(root: &Path, field: &str, alt: Option<&str>, dst: &mut FixedStr<N>) {
    dst.set("");
    let candidates = [
        root.join("sys/class/dmi/id").join(field),
        root.join("sys/devices/virtual/dmi/id").join(field),
    ];
    let alt = alt.map(|a| root.join(a.trim_start_matches('/')));
    let Some(path) = candidates.iter().chain(alt.as_ref()).find(|p| readable(p)) else {
        return;
    };
    let Some(text) = crate::system::read_txt_file(path, 256) else {
        return;
    };
    dst.set(clean_field(&text));
}

/// `fill_dmi_info()`: the DMI texts read anew, then the product named from them (the host prefix is not set yet).
pub(super) fn fill(ds: &mut super::StatusFile) {
    ds.hw = read(Path::new("/"));
    super::product::normalize(ds, "");
}

/// `os_dmi_info_get()` under `root`, in C's order.
pub(super) fn read(root: &Path) -> Dmi {
    let mut d = Dmi::default();
    read_field(root, "sys_vendor", None, &mut d.sys_vendor);
    read_field(root, "product_uuid", None, &mut d.sys_uuid);
    read_field(root, "product_name", Some("/proc/device-tree/model"), &mut d.product_id);
    if !d.product_id.is_empty() {
        d.product_name.set(d.product_id.as_bytes());
    }
    read_field(root, "product_version", None, &mut d.product_version);
    read_field(root, "product_sku", None, &mut d.product_sku);
    read_field(root, "product_family", None, &mut d.product_family);
    read_field(root, "chassis_vendor", None, &mut d.chassis_vendor);
    read_field(root, "chassis_version", None, &mut d.chassis_version);
    read_field(root, "board_vendor", None, &mut d.board_vendor);
    read_field(root, "board_name", None, &mut d.board_name);
    read_field(root, "board_version", None, &mut d.board_version);
    read_field(root, "bios_vendor", None, &mut d.bios_vendor);
    read_field(root, "bios_version", None, &mut d.bios_version);
    read_field(root, "bios_date", None, &mut d.bios_date);
    read_field(root, "bios_release", None, &mut d.bios_release);
    read_field(root, "chassis_type", None, &mut d.chassis_type);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_fields_as_c() {
        for (raw, want) in [
            (&b"QEMU\n"[..], &b"QEMU"[..]),
            (b"\n", b""),
            (b"---", b""),
            (b"A\tB\x01C", b"A B C"),
            (b"  Caf\xc3\xa9   Bar ", b"Caf Bar"),
            (b"  to be filled by o.e.m. ", b""),
            (b"OEMX", b"OEMX"),
            (b"sku", b""),
            (b"Model\0junk", b"Model"),
        ] {
            assert_eq!(clean_field(raw), want, "{}", String::from_utf8_lossy(raw));
        }
    }

    #[test]
    fn reads_fields_as_c() {
        let root = tempfile::tempdir().unwrap();
        let put = |rel: &str, text: &[u8]| {
            let path = root.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put("sys/class/dmi/id/sys_vendor", b"QEMU\n");
        // the virtual path when the class one is missing
        put("sys/devices/virtual/dmi/id/board_vendor", b"Board Co\n");
        // a device-tree model, NUL-terminated, when there is no product name
        put("proc/device-tree/model", b"Raspberry Pi 4 Model B Rev 1.4\0");
        // cleaned before the cut: 70 letters become 63, a cut space stays
        put(
            "sys/class/dmi/id/product_family",
            format!("{} {}", "f".repeat(62), "x".repeat(7)).as_bytes(),
        );
        // only 255 bytes are read
        put("sys/class/dmi/id/bios_version", &[b'v'; 300]);
        put("sys/class/dmi/id/chassis_type", b"10\n");
        let d = read(root.path());
        assert_eq!(d.sys_vendor.as_bytes(), b"QEMU");
        assert_eq!(d.board_vendor.as_bytes(), b"Board Co");
        assert_eq!(d.product_id.as_bytes(), b"Raspberry Pi 4 Model B Rev 1.4");
        assert_eq!(d.product_name.as_bytes(), d.product_id.as_bytes());
        assert_eq!(d.product_family.as_bytes(), format!("{} ", "f".repeat(62)).as_bytes());
        assert_eq!(d.bios_version.as_bytes(), &[b'v'; 63][..]);
        assert_eq!(d.chassis_type.as_bytes(), b"10");
        assert!(d.sys_uuid.is_empty() && d.board_name.is_empty());
        // the product name is a copy of the id: 63 bytes at most, not 95
        put("sys/class/dmi/id/product_name", "p".repeat(90).as_bytes());
        assert_eq!(read(root.path()).product_name.as_bytes().len(), 63);
    }
}
