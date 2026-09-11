//! 浏览器扩展的内嵌分发。
//!
//! 扩展文件在编译期用 `include_str!` / `include_bytes!` 打进二进制，运行时按需打包成 zip，
//! 这样即使用户只拿到单个可执行文件（没有源码目录），也能从面板「下载扩展」拿到可用的一键登录扩展。
//!
//! zip 采用 **stored（不压缩）** 模式手写：扩展总共几十 KB，省掉一个压缩库依赖更划算。

/// 文本类文件（zip 内路径 → 内容）
const TEXT_FILES: &[(&str, &str)] = &[
    ("manifest.json", include_str!("../browser-extension/manifest.json")),
    ("background.js", include_str!("../browser-extension/background.js")),
    ("bridge.js", include_str!("../browser-extension/bridge.js")),
    ("options.html", include_str!("../browser-extension/options.html")),
    ("options.js", include_str!("../browser-extension/options.js")),
    ("README.md", include_str!("../browser-extension/README.md")),
];

/// 二进制类文件
const BIN_FILES: &[(&str, &[u8])] = &[(
    "icons/icon128.png",
    include_bytes!("../browser-extension/icons/icon128.png"),
)];

/// 扩展版本号（从内嵌 manifest.json 解析，避免和扩展本体不同步）
pub fn version() -> String {
    let manifest = TEXT_FILES
        .iter()
        .find(|(n, _)| *n == "manifest.json")
        .map(|(_, c)| *c)
        .unwrap_or("");
    regex::Regex::new(r#""version"\s*:\s*"([^"]+)""#)
        .ok()
        .and_then(|re| re.captures(manifest))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// CRC-32（IEEE 802.3，zip 要求）；逐位实现，几十 KB 数据量下开销可忽略
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// 当前时间的 DOS 时间戳 (time, date)
fn dos_datetime() -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let year = (now.year().clamp(1980, 2107) - 1980) as u16;
    let date = (year << 9) | ((now.month() as u16) << 5) | (now.day() as u16);
    let time = ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | ((now.second() as u16) / 2);
    (time, date)
}

/// 把内嵌的扩展文件打包成 zip（stored 模式）
pub fn build_zip() -> Vec<u8> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, content) in TEXT_FILES {
        entries.push(((*name).to_string(), content.as_bytes().to_vec()));
    }
    for (name, bytes) in BIN_FILES {
        entries.push(((*name).to_string(), bytes.to_vec()));
    }

    let (dos_time, dos_date) = dos_datetime();
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let count = entries.len() as u16;

    for (name, data) in &entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let size = data.len() as u32;
        let local_offset = out.len() as u32;

        // ---- Local file header ----
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method 0 = stored
        out.extend_from_slice(&dos_time.to_le_bytes());
        out.extend_from_slice(&dos_date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        // ---- Central directory entry ----
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&dos_time.to_le_bytes());
        central.extend_from_slice(&dos_date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&local_offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }

    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);

    // ---- End of central directory ----
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn zip_has_magic_and_eocd() {
        let z = build_zip();
        assert!(z.len() > 1000, "zip 太小，可能没打进扩展文件");
        assert_eq!(&z[0..4], &[0x50, 0x4b, 0x03, 0x04], "缺少 zip 局部文件头魔数");
        let eocd = &z[z.len() - 22..];
        assert_eq!(&eocd[0..4], &[0x50, 0x4b, 0x05, 0x06], "缺少 EOCD 魔数");
    }

    #[test]
    fn zip_contains_every_extension_file() {
        let z = build_zip();
        let text = String::from_utf8_lossy(&z);
        for (name, _) in TEXT_FILES {
            assert!(text.contains(name), "zip 内缺少 {name}");
        }
        assert!(text.contains("icons/icon128.png"), "zip 内缺少图标");
    }

    #[test]
    fn zip_entry_count_matches() {
        let z = build_zip();
        let eocd = &z[z.len() - 22..];
        let total = u16::from_le_bytes([eocd[10], eocd[11]]);
        assert_eq!(total as usize, TEXT_FILES.len() + BIN_FILES.len());
    }

    #[test]
    fn version_is_parsed_from_manifest() {
        let v = version();
        assert_ne!(v, "unknown", "未能从内嵌 manifest.json 解析出版本号");
        assert!(v.chars().next().unwrap().is_ascii_digit(), "版本号格式异常: {v}");
    }
}
