use crate::Guid;

pub fn mbr_type_name(t: u8) -> &'static str {
    match t {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x05 | 0x0F | 0x85 => "Extended",
        0x07 => "NTFS / exFAT",
        0x0B | 0x0C => "FAT32",
        0x11 | 0x14 | 0x16 | 0x1B | 0x1C | 0x1E => "Hidden FAT",
        0x17 => "Hidden NTFS / exFAT",
        0x27 => "Windows recovery",
        0x42 => "Windows dynamic disk",
        0x82 => "Linux swap",
        0x83 => "Linux",
        0x8E => "Linux LVM",
        0xA5 | 0xA6 | 0xA9 => "BSD",
        0xAB => "Apple boot",
        0xAF => "Apple HFS / HFS+",
        0xEE => "GPT protective",
        0xEF => "EFI system",
        0xFD => "Linux RAID",
        _ => "Unknown",
    }
}

pub fn is_extended(t: u8) -> bool {
    matches!(t, 0x05 | 0x0F | 0x85)
}

const GPT_TYPES: &[(&str, &str)] = &[
    ("C12A7328-F81F-11D2-BA4B-00A0C93EC93B", "EFI system"),
    ("E3C9E316-0B5C-4DB8-817D-F92DF00215AE", "Microsoft reserved"),
    ("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", "Microsoft basic data"),
    ("DE94BBA4-06D1-4D40-A16A-BFD50179D6AC", "Windows recovery"),
    ("5808C8AA-7E8F-42E0-85D2-E1E90434CFB3", "Windows LDM metadata"),
    ("AF9B60A0-1431-4F62-BC68-3311714A69AD", "Windows LDM data"),
    ("E75CAF8F-F680-4CEE-AFA3-B001E56EFC2D", "Storage Spaces"),
    ("7C3457EF-0000-11AA-AA11-00306543ECAC", "Apple APFS"),
    ("48465300-0000-11AA-AA11-00306543ECAC", "Apple HFS+"),
    ("426F6F74-0000-11AA-AA11-00306543ECAC", "Apple boot"),
    ("52637672-7900-11AA-AA11-00306543ECAC", "Apple recovery"),
    ("0FC63DAF-8483-4772-8E79-3D69D8477DE4", "Linux filesystem"),
    ("0657FD6D-A4AB-43C4-84E5-0933C84B4F4F", "Linux swap"),
    ("E6D6D379-F507-44C2-A23C-238F2A3DF928", "Linux LVM"),
    ("21686148-6449-6E6F-744E-656564454649", "BIOS boot"),
];

pub fn gpt_type_name(g: &Guid) -> String {
    GPT_TYPES
        .iter()
        .find(|(text, _)| Guid::parse(text).as_ref() == Some(g))
        .map_or_else(|| format!("Unknown ({g})"), |(_, name)| (*name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(mbr_type_name(0x0C), "FAT32");
        assert!(is_extended(0x0F));
        assert!(!is_extended(0x07));
        let basic = Guid::parse("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7").unwrap();
        assert_eq!(gpt_type_name(&basic), "Microsoft basic data");
        assert!(gpt_type_name(&Guid([7; 16])).starts_with("Unknown"));
    }
}
