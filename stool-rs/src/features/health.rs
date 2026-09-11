//! D1「游戏为什么跑不起来」体检（P2-8）。
//!
//! 逐项给「结论 + 现象 + 修法」：主程序 / 路径 / 系统区域设置 / 日文字体 /
//! 运行库 DLL / 写权限。检测全部只读，不改动游戏。
//!
//! 复用 `features::precheck` 的 `Item / Level / Report`，保证与预检同一套展示。

use std::path::{Path, PathBuf};

use crate::features::precheck::{probe_writable, Item, Level, Report};

/// 中文 Windows 上"能正常显示日文游戏"通常需要区域设置或转区工具。
const FONT_DIR: &str = "C:/Windows/Fonts";

/// 体检入口。`engine_id` 可传空串（未知引擎时按结构特征判断）。
pub fn check(root: &Path, engine_id: &str) -> Report {
    let mut items = Vec::new();
    if !root.is_dir() {
        items.push(Item {
            name: "游戏目录",
            level: Level::Fail,
            detail: format!("不存在或不是目录：{}", root.display()),
            fix: Some("在首页重新选择有效的游戏根目录".into()),
        });
        return Report { items };
    }

    let files = root_files(root);

    // 1) 主程序
    let exe = files.iter().find(|(n, _)| n.ends_with(".exe")).map(|(_, p)| p.clone());
    match &exe {
        Some(e) => items.push(Item {
            name: "主程序",
            level: Level::Ok,
            detail: e.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            fix: None,
        }),
        None => items.push(Item {
            name: "主程序",
            level: Level::Warn,
            detail: "根目录没有 .exe".into(),
            fix: Some("可能选到了数据目录；请选择含游戏主程序的文件夹".into()),
        }),
    }

    // 2) 路径：老游戏对非 ASCII / 超长路径很敏感
    let p = root.to_string_lossy().into_owned();
    if !p.is_ascii() {
        items.push(Item {
            name: "路径字符",
            level: Level::Warn,
            detail: "路径含非 ASCII 字符（中文/日文）".into(),
            fix: Some("老引擎可能读不到；把游戏移到纯英文短路径（如 D:\\games\\xxx）".into()),
        });
    }
    if p.chars().count() > 180 {
        items.push(Item {
            name: "路径长度",
            level: Level::Warn,
            detail: format!("{} 个字符，接近 Windows 上限", p.chars().count()),
            fix: Some("移到更短的路径以免写盘失败".into()),
        });
    }

    // 3) 系统区域设置（非 Unicode 程序语言 / ACP）
    match ansi_codepage() {
        Some(acp) => {
            let (level, desc, fix) = judge_codepage(acp);
            items.push(Item { name: "系统区域（非 Unicode 程序）", level, detail: desc, fix });
        }
        None => items.push(Item {
            name: "系统区域（非 Unicode 程序）",
            level: Level::Ok,
            detail: "未能读取（非 Windows 或键值缺失），跳过".into(),
            fix: None,
        }),
    }

    // 4) 日文字体（缺了会显示成方块 / 空白）
    let jp_fonts: Vec<&str> = [("msgothic.ttc", "MS Gothic"), ("meiryo.ttc", "Meiryo"), ("YuGothM.ttc", "Yu Gothic")]
        .iter()
        .filter(|(f, _)| Path::new(FONT_DIR).join(f).exists())
        .map(|(_, n)| *n)
        .collect();
    if jp_fonts.is_empty() {
        items.push(Item {
            name: "日文字体",
            level: Level::Warn,
            detail: "未检测到常用日文字体（MS Gothic / Meiryo / Yu Gothic）".into(),
            fix: Some("安装日文字体包，否则日文文本可能显示为方块".into()),
        });
    } else {
        items.push(Item {
            name: "日文字体",
            level: Level::Ok,
            detail: format!("已安装 {}", jp_fonts.join(" / ")),
            fix: None,
        });
    }

    // 5) 运行库 DLL（按结构特征判断，避免依赖具体引擎 id）
    let has_rgss_dll = files.iter().any(|(n, _)| n.starts_with("rgss") && n.ends_with(".dll"));
    let rgss_game = files
        .iter()
        .any(|(n, _)| n.ends_with(".rgss3a") || n.ends_with(".rgssad") || n.ends_with(".rgss2a") || n.ends_with(".rvdata2"));
    if rgss_game && !has_rgss_dll {
        items.push(Item {
            name: "运行库 DLL",
            level: Level::Warn,
            detail: "疑似 RPG Maker (RGSS) 游戏，但同目录缺少 RGSS*.dll".into(),
            fix: Some("安装对应版本的 RPG Maker RTP，或补齐 RGSSxxx.dll".into()),
        });
    }
    let unity_data = has_dir_ending_with(root, "_data");
    let has_unity_player = files.iter().any(|(n, _)| n == "unityplayer.dll");
    if (unity_data || engine_id == "unity") && !has_unity_player {
        items.push(Item {
            name: "运行库 DLL",
            level: Level::Warn,
            detail: "疑似 Unity 游戏，但缺少 UnityPlayer.dll".into(),
            fix: Some("游戏文件不完整，建议重新下载 / 校验完整性".into()),
        });
    }

    // 6) 写权限（存档 / 补丁要写回游戏目录）
    match probe_writable(root) {
        Ok(()) => items.push(Item { name: "写权限", level: Level::Ok, detail: "游戏目录可写".into(), fix: None }),
        Err(e) => items.push(Item {
            name: "写权限",
            level: Level::Warn,
            detail: format!("游戏目录不可写（{}）", e),
            fix: Some("装到 Program Files 时常见；把游戏移到用户可写目录，或以管理员运行".into()),
        }),
    }

    Report { items }
}

fn judge_codepage(acp: u32) -> (Level, String, Option<String>) {
    match acp {
        936 => (Level::Ok, "简体中文（ACP=936）".into(), None),
        65001 => (Level::Ok, "UTF-8（ACP=65001）".into(), None),
        932 => (Level::Ok, "日文（ACP=932），日文游戏兼容良好".into(), None),
        other => (
            Level::Warn,
            format!("当前 ACP={other}（既非中文/日文也非 UTF-8）"),
            Some("用 Locale Emulator 运行，或把「非 Unicode 程序语言」设为日文/中文".into()),
        ),
    }
}

fn root_files(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
        .collect()
}

fn has_dir_ending_with(root: &Path, suffix: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else { return false };
    rd.flatten().any(|e| {
        e.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && e.file_name().to_string_lossy().to_lowercase().ends_with(suffix)
    })
}

/// 读取系统「非 Unicode 程序」代码页（HKLM\SYSTEM\...\Nls\CodePage\ACP）。
#[cfg(windows)]
fn ansi_codepage() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let sub = wide("SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage");
    let val = wide("ACP");
    let mut buf = [0u16; 64];
    let mut len = (buf.len() * std::mem::size_of::<u16>()) as u32;
    // SAFETY: subkey/value 均为 NUL 结尾宽字符串；buf 有界，len 为其字节容量。
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut len,
        )
    };
    if rc != 0 {
        return None;
    }
    let n = (len as usize / std::mem::size_of::<u16>()).saturating_sub(1);
    String::from_utf16_lossy(&buf[..n.min(buf.len())]).trim().parse().ok()
}

#[cfg(not(windows))]
fn ansi_codepage() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_health_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_dir_is_fail() {
        let r = check(Path::new("Z:/nope_stool_health"), "");
        assert!(!r.ok());
        assert_eq!(r.first_fail().unwrap().name, "游戏目录");
    }

    #[test]
    fn empty_dir_warns_no_exe() {
        let d = tmp("empty");
        let r = check(&d, "");
        assert!(r.ok(), "只有告警不应阻塞：{:?}", r.lines());
        assert!(r.lines().iter().any(|l| l.contains("主程序")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rgss_without_dll_warns() {
        let d = tmp("rgss");
        fs::write(d.join("Game.exe"), b"x").unwrap();
        fs::write(d.join("Game.rgss3a"), b"y").unwrap(); // 有 rgss 封包但无 RGSS*.dll
        let r = check(&d, "rpgmaker_rgss");
        let has = r.items.iter().any(|i| i.name == "运行库 DLL" && i.level == Level::Warn);
        assert!(has, "应提示缺少 RGSS*.dll：{:?}", r.lines());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rgss_with_dll_ok() {
        let d = tmp("rgssok");
        fs::write(d.join("Game.exe"), b"x").unwrap();
        fs::write(d.join("Game.rgss3a"), b"y").unwrap();
        fs::write(d.join("RGSS300.dll"), b"z").unwrap();
        let r = check(&d, "rpgmaker_rgss");
        assert!(!r.items.iter().any(|i| i.name == "运行库 DLL"), "有 DLL 时不该出现缺失提示");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unity_without_player_dll_warns() {
        let d = tmp("unity");
        fs::create_dir_all(d.join("MyGame_Data")).unwrap();
        let r = check(&d, "unity");
        let has = r.items.iter().any(|i| i.name == "运行库 DLL" && i.detail.contains("UnityPlayer"));
        assert!(has, "应提示缺少 UnityPlayer.dll：{:?}", r.lines());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn codepage_judgement() {
        assert_eq!(judge_codepage(936).0, Level::Ok);
        assert_eq!(judge_codepage(65001).0, Level::Ok);
        assert_eq!(judge_codepage(932).0, Level::Ok);
        assert_eq!(judge_codepage(1252).0, Level::Warn);
        assert!(judge_codepage(1252).2.is_some(), "非常用区域应给修法");
    }
}
