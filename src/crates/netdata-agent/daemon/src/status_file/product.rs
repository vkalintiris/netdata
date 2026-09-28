//! `status-file-product.c`: the product the record names from its DMI texts, cloud and virtualization: vendor, id,
//! name and type. The vendor is never reset: a recompute without any DMI vendor keeps the one it had, as in C.

use std::collections::HashSet;
use std::path::Path;

use netdata_agent_text::c::{eq_ignore_case, find_ignore_case};
use netdata_agent_text::parse::strtoll10;

use super::{Dmi, FixedStr, StatusFile};

/// `dmi_normalize_vendor_field()`'s table, generated from `status-file-product.c`: the first exact, case-insensitive
/// match wins.
const VENDORS: [(&str, &str); 111] = [
    ("QEMU", "KVM"),
    ("AMD Corporation", "AMD"),
    ("Advanced Micro Devices, Inc.", "AMD"),
    ("AMI Corp.", "AMI"),
    ("AMI Corporation", "AMI"),
    ("American Megatrends", "AMI"),
    ("American Megatrends Inc.", "AMI"),
    ("American Megatrends International", "AMI"),
    ("American Megatrends International, LLC.", "AMI"),
    ("AOPEN", "AOpen"),
    ("AOPEN Inc.", "AOpen"),
    ("Apache Software Foundation", "Apache"),
    ("Apple Inc.", "Apple"),
    ("ASRock Industrial", "ASRock"),
    ("ASRockRack", "ASRock"),
    ("AsrockRack", "ASRock"),
    ("ASUS", "ASUSTeK"),
    ("ASUSTeK COMPUTER INC.", "ASUSTeK"),
    ("ASUSTeK COMPUTER INC. (Licensed from AMI)", "ASUSTeK"),
    ("ASUSTeK Computer INC.", "ASUSTeK"),
    ("ASUSTeK Computer Inc.", "ASUSTeK"),
    ("ASUSTek Computer INC.", "ASUSTeK"),
    ("Apache Software Foundation", "Apache"),
    ("BESSTAR (HK) LIMITED", "Besstar"),
    ("BESSTAR TECH", "Besstar"),
    ("BESSTAR TECH LIMITED", "Besstar"),
    ("BESSTAR Tech", "Besstar"),
    ("CHUWI", "Chuwi"),
    ("CHUWI Innovation And Technology(ShenZhen)co.,Ltd", "Chuwi"),
    ("Cisco Systems Inc", "Cisco"),
    ("Cisco Systems, Inc.", "Cisco"),
    ("DELL", "Dell"),
    ("Dell Computer Corporation", "Dell"),
    ("Dell Inc.", "Dell"),
    ("Dell EMC", "Dell"),
    ("FUJITSU", "Fujitsu"),
    ("FUJITSU CLIENT COMPUTING LIMITED", "Fujitsu"),
    ("FUJITSU SIEMENS", "Fujitsu"),
    ("FUJITSU SIEMENS // Phoenix Technologies Ltd.", "Fujitsu"),
    ("FUJITSU // American Megatrends Inc.", "Fujitsu"),
    ("FUJITSU // American Megatrends International, LLC.", "Fujitsu"),
    ("FUJITSU // Insyde Software Corp.", "Fujitsu"),
    ("FUJITSU // Phoenix Technologies Ltd.", "Fujitsu"),
    ("GIGABYTE", "Gigabyte"),
    ("Giga Computing", "Gigabyte"),
    ("Gigabyte Technology Co., Ltd.", "Gigabyte"),
    ("Gigabyte Tecohnology Co., Ltd.", "Gigabyte"),
    ("GOOGLE", "Google"),
    ("Google Inc", "Google"),
    ("HC Technology.,Ltd.", "HC Tech"),
    ("HP-Pavilion", "HP"),
    ("HPE", "HP"),
    ("Hewlett Packard Enterprise", "HP"),
    ("Hewlett-Packard", "HP"),
    ("HUAWEI", "Huawei"),
    ("Huawei Technologies Co., Ltd.", "Huawei"),
    ("IBM Corp.", "IBM"),
    ("IceWhale Technology Co.,Ltd.", "IceWhale"),
    ("INSYDE", "Insyde"),
    ("INSYDE Corp.", "Insyde"),
    ("Insyde Corp.", "Insyde"),
    ("INTEL", "Intel"),
    ("INTEL Corporation", "Intel"),
    ("Intel Corp.", "Intel"),
    ("Intel Corporation", "Intel"),
    ("Intel corporation", "Intel"),
    ("Intel(R) Client Systems", "Intel"),
    ("Intel(R) Corporation", "Intel"),
    ("LENOVO", "Lenovo"),
    ("LNVO", "Lenovo"),
    ("Shenzhen Meigao Electronic Equipment Co.,Ltd", "Meigao"),
    ("Micro Computer (HK) Tech Limited", "Micro Computer"),
    ("Micro Computer(HK) Tech Limited", "Micro Computer"),
    ("MICRO-STAR INTERNATIONAL CO., LTD", "MSI"),
    ("MICRO-STAR INTERNATIONAL CO.,LTD", "MSI"),
    ("MSI", "MSI"),
    ("Micro-Star International Co., Ltd", "MSI"),
    ("Micro-Star International Co., Ltd.", "MSI"),
    ("MICROSOFT", "Microsoft"),
    ("Microsoft Corporation", "Microsoft"),
    ("nVIDIA", "NVIDIA"),
    ("OPENSTACK", "OpenStack"),
    ("OpenStack Foundation", "OpenStack"),
    ("ORACLE CORPORATI", "Oracle"),
    ("Oracle Corporation", "Oracle"),
    ("innotek GmbH", "Oracle"),
    ("Phoenix Technologies LTD", "Phoenix"),
    ("Phoenix Technologies Ltd", "Phoenix"),
    ("Phoenix Technologies Ltd.", "Phoenix"),
    ("Phoenix Technologies, LTD", "Phoenix"),
    ("QNAP Systems, Inc.", "QNAP"),
    ("QUANTA", "Quanta"),
    ("Quanta Cloud Technology Inc.", "Quanta"),
    ("Quanta Computer Inc", "Quanta"),
    ("Quanta Computer Inc.", "Quanta"),
    ("RED HAT", "Red Hat"),
    ("SAMSUNG ELECTRONICS CO., LTD.", "Samsung"),
    ("SUN MICROSYSTEMS", "Sun"),
    ("SuperMicro", "Supermicro"),
    ("Supermicro Corporation", "Supermicro"),
    ("SYNOLOGY", "Synology"),
    ("Synology Inc.", "Synology"),
    ("TYAN", "Tyan"),
    ("TYAN Computer Corporation", "Tyan"),
    ("Tyan Computer Corporation", "Tyan"),
    ("$(TYAN_SYSTEM_MANUFACTURER)", "Tyan"),
    ("VMware", "VMware"),
    ("VMware, Inc.", "VMware"),
    ("XIAOMI", "Xiaomi"),
    ("ZOTAC", "Zotac"),
    ("Motherboard by ZOTAC", "Zotac"),
];

/// `strcasestr()` is not NULL; an empty needle is found, an empty haystack finds only it.
fn contains(haystack: &[u8], needle: &str) -> bool {
    find_ignore_case(haystack, needle.as_bytes()).is_some()
}

/// `dmi_normalize_vendor_field()`.
fn normalize_vendor(vendor: &mut FixedStr<64>) {
    if let Some((_, to)) = VENDORS
        .iter()
        .find(|(from, _)| eq_ignore_case(vendor.as_bytes(), from.as_bytes()))
    {
        vendor.set(to);
    }
}

/// `dmi_is_virtual_machine()`.
fn is_virtual_machine(d: &Dmi) -> bool {
    const INDICATORS: [&str; 18] = [
        "Virt",
        "KVM",
        "vServer",
        "Cloud",
        "Hyper",
        "Droplet",
        // with a space, not to match "Computer"
        "Compute ",
        "HVM domU",
        "Parallels",
        "(i440FX",
        "(q35",
        "OpenStack",
        "QEMU",
        "VMWare",
        "DigitalOcean",
        "Oracle",
        "Linode",
        "Amazon EC2",
    ];
    [
        d.product_id.as_bytes(),
        d.product_name.as_bytes(),
        d.product_family.as_bytes(),
        d.sys_vendor.as_bytes(),
        d.board_name.as_bytes(),
    ]
    .iter()
    .filter(|f| !f.is_empty())
    .any(|f| INDICATORS.iter().any(|i| contains(f, i)))
}

/// `dmi_any_field_contains_any()`: the product id, name and family, the board name and the system vendor.
fn any_field_contains_any(d: &Dmi, needles: &[&str]) -> bool {
    [
        d.product_id.as_bytes(),
        d.product_name.as_bytes(),
        d.product_family.as_bytes(),
        d.board_name.as_bytes(),
        d.sys_vendor.as_bytes(),
    ]
    .iter()
    .filter(|f| !f.is_empty())
    .any(|f| needles.iter().any(|n| contains(f, n)))
}

/// `dmi_field_starts_with_mac()`.
fn starts_with_mac(value: &[u8]) -> bool {
    value.len() >= 3 && eq_ignore_case(&value[..3], b"Mac")
}

/// `dmi_is_apple_product()`.
fn is_apple(ds: &StatusFile) -> bool {
    contains(ds.product.vendor.as_bytes(), "Apple")
        || contains(ds.hw.sys_vendor.as_bytes(), "Apple")
        || contains(ds.hw.board_vendor.as_bytes(), "Apple")
        || starts_with_mac(ds.hw.product_id.as_bytes())
        || starts_with_mac(ds.hw.product_name.as_bytes())
}

/// `dmi_is_server_product_line()`.
fn is_server_line(d: &Dmi) -> bool {
    any_field_contains_any(
        d,
        &[
            "Server",
            "PowerEdge",
            "ProLiant",
            "ThinkSystem",
            "PRIMERGY",
            "System x",
            "BladeCenter",
            "RackStation",
        ],
    )
}

/// `dmi_is_workstation_product_line()`: an Apple product only as a Mac Studio or a Mac Pro.
fn is_workstation_line(ds: &StatusFile) -> bool {
    if is_apple(ds) {
        let name = ds.hw.product_name.as_bytes();
        return contains(name, "Mac Studio") || contains(name, "Mac Pro") || contains(name, "iMac Pro");
    }
    any_field_contains_any(
        &ds.hw,
        &[
            "workstation",
            "ThinkStation",
            "Precision Workstation",
            "Z Workstation",
            "Pro Workstation",
        ],
    )
}

/// `dmi_is_laptop_product_line()`.
fn is_laptop_line(ds: &StatusFile) -> bool {
    any_field_contains_any(&ds.hw, &["MacBook", "notebook", "laptop"])
}

/// `dmi_chassis_type_to_string()`: SMBIOS chassis types in five classes.
fn chassis_class(chassis_type: i32) -> &'static str {
    match chassis_type {
        3 | 4 | 6 | 7 | 13 | 15 | 24 | 26 => "desktop",
        5 | 8..=12 | 14 | 16 | 30..=32 => "laptop",
        17 | 23 | 25 | 27..=29 => "server",
        33..=36 => "mini-pc",
        _ => "unknown",
    }
}

/// Entries of `dir` named `prefix` and a digit, then anything.
fn numbered(dir: &Path, prefix: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.as_encoded_bytes();
            name.starts_with(prefix.as_bytes()) && name.get(prefix.len()).is_some_and(u8::is_ascii_digit)
        })
        .map(|e| e.path())
        .collect()
}

/// `is_server_hardware()` under the host prefix: ECC memory controllers, an IPMI device, or CPUs in more than one
/// package (their `physical_package_id` texts, newline and all, as C compares them).
fn is_server_hardware(prefix: &str) -> bool {
    let root = Path::new(if prefix.is_empty() { "/" } else { prefix });
    let ecc = numbered(&root.join("sys/devices/system/edac/mc"), "mc")
        .iter()
        .any(|mc| mc.join("ce_count").exists());
    if ecc || root.join("dev/ipmi0").exists() {
        return true;
    }
    let packages: HashSet<Vec<u8>> = numbered(&root.join("sys/devices/system/cpu"), "cpu")
        .iter()
        .filter_map(|cpu| crate::system::read_txt_file(cpu.join("topology/physical_package_id"), 64))
        .filter(|id| !id.is_empty())
        .collect();
    packages.len() > 1
}

/// C's `strcatz()` into the product name at `len`: what fits of `text`, the new length.
fn append(name: &mut FixedStr<96>, len: usize, text: &[u8]) -> usize {
    let mut value = name.as_bytes()[..len.min(name.as_bytes().len())].to_vec();
    value.extend_from_slice(text);
    name.set(&value);
    name.as_bytes().len()
}

/// `product_name_vendor_type()`; `prefix` is the host prefix the server hardware probes use.
pub(super) fn normalize(ds: &mut StatusFile, prefix: &str) {
    let mut force_type = None;
    let hw = ds.hw;
    let known = |text: &FixedStr<32>| !text.is_empty() && !eq_ignore_case(text.as_bytes(), b"unknown");

    if !hw.product_id.is_empty() {
        ds.product.id.set(hw.product_id.as_bytes());
    } else {
        ds.product.id.set(hw.product_name.as_bytes());
    }

    if known(&ds.cloud_provider_type) {
        ds.product.vendor.set(ds.cloud_provider_type.as_bytes());
    } else {
        // a DMI vendor; without one, the vendor stays what it was
        if let Some(v) = [hw.sys_vendor, hw.board_vendor, hw.chassis_vendor, hw.bios_vendor]
            .iter()
            .find(|v| !v.is_empty())
        {
            ds.product.vendor.set(v.as_bytes());
        }
        if ds.product.vendor.is_empty() {
            let (id, name, board) = (
                hw.product_id.as_bytes(),
                hw.product_name.as_bytes(),
                hw.board_name.as_bytes(),
            );
            let derived = if contains(id, "VirtualMac")
                || contains(name, "VirtualMac")
                || (contains(board, "Apple") && contains(board, "Virtual"))
            {
                Some(("Apple", "vm"))
            } else if contains(name, "NVIDIA") && contains(name, "Kit") {
                Some(("NVIDIA", "mini-pc"))
            } else if contains(name, "Raspberry") {
                Some(("Raspberry", "mini-pc"))
            } else if contains(name, "ODROID") {
                Some(("Odroid", "mini-pc"))
            } else if contains(name, "BananaPi") || contains(name, "Banana Pi") {
                Some(("BananaPi", "mini-pc"))
            } else if contains(name, "OrangePi") || contains(name, "Orange Pi") {
                Some(("OrangePi", "mini-pc"))
            } else {
                None
            };
            if let Some((vendor, kind)) = derived {
                ds.product.vendor.set(vendor);
                force_type = Some(kind);
            }
        }
        if ds.product.vendor.is_empty() {
            ds.product.vendor.set("unknown");
        } else {
            normalize_vendor(&mut ds.product.vendor);
        }
    }
    let vendor = ds.product.vendor.as_bytes();
    if force_type.is_none()
        && ["Raspberry", "BananaPi", "OrangePi", "ODROID"]
            .iter()
            .any(|v| contains(vendor, v))
    {
        force_type = Some("mini-pc");
    }

    if known(&ds.cloud_instance_type) {
        ds.product.name.set(ds.cloud_instance_type.as_bytes());
    } else {
        let (family, name, board) = (
            hw.product_family.as_bytes(),
            hw.product_name.as_bytes(),
            hw.board_name.as_bytes(),
        );
        let mut len = append(&mut ds.product.name, 0, family);
        let enriched = !hw.product_id.is_empty() && !name.is_empty() && hw.product_id.as_bytes() != name;
        if !name.is_empty() && find_ignore_case(ds.product.name.as_bytes(), name).is_none() {
            if !ds.product.name.is_empty() {
                len = if family.is_empty() || find_ignore_case(name, family).is_some() {
                    0
                } else {
                    append(&mut ds.product.name, len, b" / ")
                };
            }
            len = append(&mut ds.product.name, len, name);
        }
        if !enriched && !board.is_empty() && find_ignore_case(ds.product.name.as_bytes(), board).is_none() {
            if !ds.product.name.is_empty() {
                let has_family = family.is_empty() || find_ignore_case(board, family).is_some();
                let has_product = name.is_empty() || find_ignore_case(board, name).is_some();
                len = if has_family && has_product {
                    0
                } else {
                    append(&mut ds.product.name, len, b" / ")
                };
            }
            append(&mut ds.product.name, len, board);
        }
        if ds.product.name.is_empty() {
            ds.product.name.set("unknown");
        }
    }

    // strtol(), cast to int: counted when not zero and wholly a number
    let chassis = hw.chassis_type.as_bytes();
    let (value, used, _) = strtoll10(chassis);
    let class = if value as i32 != 0 && used == chassis.len() {
        chassis_class(value as i32)
    } else {
        "unknown"
    };
    let virt = ds.virtualization.as_bytes();
    let kind = if !virt.is_empty() && !eq_ignore_case(virt, b"none") && !eq_ignore_case(virt, b"unknown") {
        "vm"
    } else if let Some(kind) = force_type {
        kind
    } else if is_virtual_machine(&hw) {
        "vm"
    } else if class == "server" || is_server_line(&hw) {
        "server"
    } else if is_workstation_line(ds) && class != "laptop" && !is_laptop_line(ds) {
        "workstation"
    } else if is_server_hardware(prefix) {
        "server"
    } else {
        class
    };
    ds.product.kind.set(kind);
}

#[cfg(test)]
mod tests {
    use super::super::{Dmi, FixedStr, StatusFile, from_json};
    use super::*;

    fn hw(set: impl FnOnce(&mut Dmi)) -> StatusFile {
        let mut ds = StatusFile::default();
        set(&mut ds.hw);
        ds
    }

    fn named(mut ds: StatusFile, prefix: &str) -> (String, String, String, String) {
        normalize(&mut ds, prefix);
        let text = |f: &[u8]| String::from_utf8_lossy(f).into_owned();
        (
            text(ds.product.vendor.as_bytes()),
            text(ds.product.id.as_bytes()),
            text(ds.product.name.as_bytes()),
            text(ds.product.kind.as_bytes()),
        )
    }

    fn no_server_hardware() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// This box's DMI as C read it: KVM, the product name twice, a VM, with or without the detected virtualization.
    #[test]
    fn names_the_oracle_box_as_c() {
        let golden = include_bytes!("../../tests/golden/status-netdata-master-prod.json");
        let mut ds = StatusFile::default();
        assert!(from_json(golden, &mut ds));
        let want = (
            ds.product.vendor.as_bytes().to_vec(),
            ds.product.name.as_bytes().to_vec(),
            ds.product.kind.as_bytes().to_vec(),
        );
        let empty = no_server_hardware();
        for virtualization in ["kvm", ""] {
            let mut fresh = StatusFile {
                hw: ds.hw,
                ..Default::default()
            };
            fresh.virtualization.set(virtualization);
            normalize(&mut fresh, empty.path().to_str().unwrap());
            let got = (
                fresh.product.vendor.as_bytes().to_vec(),
                fresh.product.name.as_bytes().to_vec(),
                fresh.product.kind.as_bytes().to_vec(),
            );
            assert_eq!(got, want, "{virtualization}");
            assert_eq!(fresh.product.id.as_bytes(), ds.product.id.as_bytes());
        }
    }

    #[test]
    fn vendors_as_c() {
        let p = no_server_hardware();
        let p = p.path().to_str().unwrap();
        // a cloud names both, unnormalized
        let mut ds = hw(|d| d.sys_vendor.set("QEMU"));
        ds.cloud_provider_type.set("AWS");
        ds.cloud_instance_type.set("t3.medium");
        assert_eq!(named(ds, p).0, "AWS");
        let mut ds = hw(|_| {});
        ds.cloud_instance_type.set("t3.medium");
        assert_eq!(named(ds, p).2, "t3.medium");
        // the first DMI vendor, normalized
        assert_eq!(named(hw(|d| d.board_vendor.set("Dell Inc.")), p).0, "Dell");
        assert_eq!(
            named(
                hw(|d| {
                    d.chassis_vendor.set("x");
                    d.bios_vendor.set("y")
                }),
                p
            )
            .0,
            "x"
        );
        assert_eq!(named(hw(|_| {}), p).0, "unknown");
        // derived, with their forced type
        for (name, vendor, kind) in [
            ("VirtualMac2,1", "Apple", "vm"),
            ("NVIDIA Jetson Nano Developer Kit", "NVIDIA", "mini-pc"),
            ("Raspberry Pi 4 Model B", "Raspberry", "mini-pc"),
            ("ODROID-N2", "Odroid", "mini-pc"),
            ("Banana Pi BPI-M5", "BananaPi", "mini-pc"),
            ("Orange Pi 5", "OrangePi", "mini-pc"),
        ] {
            let got = named(hw(|d| d.product_name.set(name)), p);
            assert_eq!((got.0.as_str(), got.3.as_str()), (vendor, kind), "{name}");
        }
    }

    /// The vendor is never reset: a second naming without a DMI vendor keeps it, and loses the derived type.
    #[test]
    fn a_recompute_keeps_the_vendor_as_c() {
        let p = no_server_hardware();
        let p = p.path().to_str().unwrap();
        for (name, kind) in [
            ("NVIDIA Jetson Nano Developer Kit", "unknown"),
            ("Raspberry Pi 4 Model B", "mini-pc"),
        ] {
            let mut ds = hw(|d| d.product_name.set(name));
            normalize(&mut ds, p);
            normalize(&mut ds, p);
            assert_eq!(ds.product.kind.as_bytes(), kind.as_bytes(), "{name}");
        }
    }

    #[test]
    fn names_products_as_c() {
        let p = no_server_hardware();
        let p = p.path().to_str().unwrap();
        let name = |family: &str, product: &str, board: &str| {
            named(
                hw(|d| {
                    d.product_family.set(family);
                    d.product_id.set(product);
                    d.product_name.set(product);
                    d.board_name.set(board);
                }),
                p,
            )
            .2
        };
        assert_eq!(
            name("ThinkPad X1", "ThinkPad X1 Carbon Gen 9", ""),
            "ThinkPad X1 Carbon Gen 9"
        );
        assert_eq!(name("Latitude", "7420", ""), "Latitude / 7420");
        assert_eq!(name("Latitude", "7420", "0XYZ"), "Latitude / 7420 / 0XYZ");
        assert_eq!(name("", "", "Z690 Board"), "Z690 Board");
        assert_eq!(name("Fam", "Fam Prod", "Fam Prod Board"), "Fam Prod Board");
        assert_eq!(name("", "", ""), "unknown");
        let long = name(&"a".repeat(60), &"b".repeat(60), "");
        assert_eq!(long.len(), 95);
        assert!(long.starts_with(&format!("{} / ", "a".repeat(60))));
    }

    #[test]
    fn types_as_c() {
        let empty = no_server_hardware();
        let p = empty.path().to_str().unwrap();
        let kind = |set: fn(&mut Dmi)| named(hw(set), p).3;
        let chassis = |t: &str| {
            let mut ds = StatusFile::default();
            ds.hw.chassis_type.set(t);
            named(ds, p).3
        };
        for (t, want) in [
            ("3", "desktop"),
            ("10", "laptop"),
            ("23", "server"),
            ("35", "mini-pc"),
            ("1", "unknown"),
            ("", "unknown"),
            ("0", "unknown"),
            ("3a", "unknown"),
            ("+3", "desktop"),
            ("4294967299", "desktop"),
        ] {
            assert_eq!(chassis(t), want, "chassis {t}");
        }
        assert_eq!(kind(|d| d.product_name.set("OpenStack Nova")), "vm");
        assert_eq!(kind(|d| d.product_name.set("PowerEdge R740")), "server");
        assert_eq!(
            kind(|d| {
                d.product_family.set("ThinkStation");
                d.chassis_type.set("3")
            }),
            "workstation"
        );
        assert_eq!(
            kind(|d| {
                d.product_family.set("ThinkStation");
                d.chassis_type.set("10")
            }),
            "laptop"
        );
        assert_eq!(
            kind(|d| {
                d.sys_vendor.set("Apple Inc.");
                d.product_name.set("Mac Pro")
            }),
            "workstation"
        );
        assert_eq!(
            kind(|d| {
                d.sys_vendor.set("Apple Inc.");
                d.product_name.set("MacBook Pro workstation")
            }),
            "unknown"
        );
        // "Computer" is not "Compute "
        assert_eq!(kind(|d| d.product_name.set("Computer")), "unknown");
        let mut ds = hw(|_| {});
        ds.virtualization.set("none");
        assert_eq!(named(ds, p).3, "unknown");
    }

    #[test]
    fn finds_server_hardware_as_c() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().to_str().unwrap();
        let put = |rel: &str, text: &str| {
            let path = root.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put("sys/devices/system/cpu/cpu0/topology/physical_package_id", "0\n");
        put("sys/devices/system/cpu/cpu1/topology/physical_package_id", "0\n");
        put("sys/devices/system/cpu/cpufreq/topology/physical_package_id", "9\n");
        assert!(!is_server_hardware(prefix));
        put("sys/devices/system/cpu/cpu2/topology/physical_package_id", "1\n");
        assert!(is_server_hardware(prefix));
        let other = tempfile::tempdir().unwrap();
        let put_in = |rel: &str| {
            let path = other.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        put_in("sys/devices/system/edac/mc/mcX/ce_count");
        assert!(!is_server_hardware(other.path().to_str().unwrap()));
        put_in("sys/devices/system/edac/mc/mc0/ce_count");
        assert!(is_server_hardware(other.path().to_str().unwrap()));
        let _ = FixedStr::<1>::default();
    }
}
