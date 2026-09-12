//! 盲区引擎识别器：表驱动，只做"识别 + 指引"，不假装有能力做处理。
//!
//! 现状：`generic.rs` 的 `KNOWN_EXT` 表里其实已经埋了 pfs→Ethornell、mjo→Mink 之类的
//! 提示，但只在"未识别"兜底文案里出现，没有任何正式插件 —— 用户看到的永远是
//! "未知引擎（兜底）"，不知道到底是哪个引擎、该走哪条路。
//!
//! 这里把那张表升级成"带分值 + 带判据 + 带处置建议"的识别规则表。每个识别器：
//! - 判据全部来自 `ScanCtx`（零额外 IO）；
//! - 分数刻意保守：**强特征单独就能过线，弱特征必须组合**，避免把 `.arc`/`.pak`
//!   这类"谁都在用"的扩展名当成唯一证据而误判；
//! - `priority` 普遍低于正式引擎（≤45），保证同分时正式引擎优先；
//! - 能力只声明 `Extract`（委派给 GARbro），并明确告知"其余请走封包回填"。

use std::path::Path;

use super::scan::ScanCtx;
use super::{Ctx, Detection, Engine, Op, OpOutcome, DETECT_LINE};

/// 单条判据。
///
/// 判据全部来自 `ScanCtx`（零额外 IO）。新增判据时**必须**同步在 [`Sig::hit`]
/// 里给出"人话证据"，GUI/CLI 直接展示这串文字。
pub enum Sig {
    /// 任意位置存在该扩展名文件
    Ext(&'static str),
    /// 任意位置存在任一扩展名
    ExtAny(&'static [&'static str]),
    /// 该扩展名文件数 ≥ n
    ExtN(&'static str, usize),
    /// 若干扩展名文件数之和 ≥ n
    ExtSum(&'static [&'static str], usize),
    /// 根目录一级存在该文件
    RootFile(&'static str),
    /// 根目录一级存在以该后缀结尾的文件
    RootFileEnds(&'static str),
    /// 根目录一级存在该目录
    RootDir(&'static str),
    /// 根目录一级存在以该后缀结尾的目录
    RootDirEnds(&'static str),
    /// 任意位置存在该目录名
    Dir(&'static str),
    /// 任意位置存在以该后缀结尾的目录名
    DirEnds(&'static str),
    /// 存在该名字的文件：根目录一级 **或** `scan::NOTABLE_FILES` 白名单（任意深度）
    FileNamed(&'static str),
    /// 根目录一级有名字包含该子串的 exe
    ExeContains(&'static str),
    /// 根目录一级有名字包含任一子串的 exe
    ExeAnyContains(&'static [&'static str]),
    /// 存在"分卷"文件（扩展名为 3 位数字，如 `x.pfs.000`）
    NumberedVol,
}

impl Sig {
    fn hit(&self, s: &ScanCtx) -> Option<String> {
        match *self {
            Sig::Ext(e) => {
                if s.has_ext(e) {
                    Some(format!("*.{e}（{} 个，如 {}）", s.ext_count(e), s.first_ext_name(e)))
                } else {
                    None
                }
            }
            Sig::ExtAny(es) => {
                for e in es {
                    if s.has_ext(e) {
                        return Some(format!("*.{e}（{} 个）", s.ext_count(e)));
                    }
                }
                None
            }
            Sig::ExtN(e, n) => {
                let c = s.ext_count(e);
                (c >= n).then(|| format!("*.{e} × {c}"))
            }
            Sig::ExtSum(es, n) => {
                let c = s.ext_sum(es);
                (c >= n).then(|| format!("{} 个封包（{}）", c, es.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join("/")))
            }
            Sig::RootFile(f) => s.has_root_file(f).then(|| format!("根目录 {f}")),
            Sig::RootFileEnds(sfx) => {
                let v = s.root_files_ending(sfx);
                (!v.is_empty()).then(|| format!("根目录 {} 类文件", sfx))
            }
            Sig::RootDir(d) => s.has_root_dir(d).then(|| format!("根目录 {d}/")),
            Sig::RootDirEnds(sfx) => {
                let v = s.root_dirs_ending(sfx);
                (!v.is_empty()).then(|| format!("根目录 {}/", v[0]))
            }
            Sig::Dir(d) => {
                if s.has_dir(d) {
                    let name = s
                        .dir_path(d)
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| d.to_string());
                    Some(format!("{name}/ 目录"))
                } else {
                    None
                }
            }
            Sig::DirEnds(sfx) => s.has_dir_ending_with(sfx).then(|| format!("*/{sfx} 目录")),
            Sig::FileNamed(f) => s.has_file_named(f).then(|| f.to_string()),
            Sig::ExeContains(sub) => {
                let n = sub.to_lowercase();
                let hit = s.exe_names().iter().find(|e| e.contains(&n)).cloned();
                hit.map(|e| e.to_string())
            }
            Sig::ExeAnyContains(subs) => {
                for sub in subs {
                    let n = sub.to_lowercase();
                    if let Some(e) = s.exe_names().iter().find(|e| e.contains(&n)).cloned() {
                        return Some(e);
                    }
                }
                None
            }
            Sig::NumberedVol => s.has_numbered_volume().then(|| "分卷文件（*.000/001）".to_string()),
        }
    }
}

pub struct Rule {
    pub pts: i32,
    pub sig: Sig,
}

#[derive(Clone, Copy)]
pub struct Rec {
    pub id: &'static str,
    pub name: &'static str,
    pub priority: i32,
    /// 处置建议（写进 notes，GUI 会显示）
    pub advice: &'static str,
    pub rules: &'static [Rule],
}

/// 判定线（与 `Detection::ok()` 共用同一常量，避免两处漂移）。
const LINE: i32 = DETECT_LINE;

// 编译期校验：下方 `LINE_HINT` 文案与测试里的「疑似线」描述都硬编码了 60 分，
// 这里用 const assert 把两者绑死——一旦 `DETECT_LINE` 变动，编译即失败。
const _: () = assert!(LINE == 60);

pub struct Recognizer(pub &'static Rec);

impl Engine for Recognizer {
    fn id(&self) -> &'static str {
        self.0.id
    }
    fn name(&self) -> &'static str {
        self.0.name
    }
    fn priority(&self) -> i32 {
        self.0.priority
    }

    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.0.id, self.0.name);
        for r in self.0.rules {
            if let Some(why) = r.sig.hit(scan) {
                d.hit(r.pts, why);
            }
        }
        if d.score > 0 {
            d.note(self.0.advice);
            if d.score < LINE {
                d.note(format!("仅为疑似（{} 分 < 判定线 {LINE}）", d.score));
            }
        }
        d
    }

    fn capabilities(&self) -> Vec<Op> {
        // 这些引擎本工具没有原生解析器，但 GARbro 大多能吃；
        // 其余能力请走"文本提取 → 翻译 → 封包回填"。
        vec![Op::Extract]
    }

    fn describe(&self, _root: &Path) -> String {
        format!("{LINE_HINT}{}", self.0.advice)
    }

    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        super::generic::garbro_extract(ctx)
    }
}

const LINE_HINT: &str = "该引擎本工具没有内置解析器，解包依赖外部 GARbro。";

use Rule as R;

// ---------------------------------------------------------------------------
// 识别规则表
//
// 打分原则：强特征（专有文件名/扩展名）单独 ≥60；弱特征（.arc/.pak/.dat 之类
// 谁都在用的）必须组合过线，否则只显示"疑似"。
// ---------------------------------------------------------------------------

pub static RECOGNIZERS: &[Rec] = &[
    Rec {
        id: "bgi_ethornell",
        name: "Ethornell / BGI（Buriko）",
        priority: 30,
        advice: "BGI 系（《夜勤病栋》《美少女万華鏡》等）。资源在 .arc / GameData/*.pack 里，\
                 可用 GARbro 解包；脚本编译进 exe，汉化一般走 解包 → 文本回填 → 重新打包。\
                 解锁优先用网盘包附带的《全CG存档》（SaveData/SystemData.dat）。",
        rules: &[
            R { pts: 60, sig: Sig::ExeAnyContains(&["bgi", "ethornell", "エンジン設定"]) },
            R { pts: 60, sig: Sig::FileNamed("bregexp.dll") },
            R { pts: 55, sig: Sig::FileNamed("bgi.gdb") },
            R { pts: 40, sig: Sig::Ext("arc") },
            R { pts: 30, sig: Sig::Dir("gamedata") },
            R { pts: 15, sig: Sig::Dir("bgm") },
        ],
    },
    Rec {
        id: "siglus",
        name: "SiglusEngine",
        priority: 32,
        advice: "SiglusEngine（Key 系《CLANNAD》《Summer Pockets》等）。\
                 数据在 .pak + Gameexe.dat，可用 GARbro 解包；文本在 .pak 内的场景脚本里。",
        rules: &[
            R { pts: 55, sig: Sig::RootFile("gameexe.dat") },
            R { pts: 30, sig: Sig::Ext("pak") },
            R { pts: 15, sig: Sig::Ext("g00") },
        ],
    },
    Rec {
        id: "reallive",
        name: "RealLive（VisualArts / Key 旧作）",
        priority: 34,
        advice: "RealLive（《Kanon》《AIR》等早期 Key 作品）。Gameexe.ini 是明文配置，\
                 资源多为 .g00/.pak；可用 GARbro 解包后回填。",
        rules: &[
            R { pts: 50, sig: Sig::RootFile("gameexe.ini") },
            R { pts: 30, sig: Sig::Ext("g00") },
            R { pts: 25, sig: Sig::RootFile("seen.txt") },
        ],
    },
    Rec {
        id: "majiro",
        name: "Majiro",
        priority: 33,
        advice: "Majiro 系。脚本为 .mjo（常封在 .arc 内），可用 GARbro 解包；\
                 文本在 .mjo 编译脚本里。",
        rules: &[
            R { pts: 65, sig: Sig::Ext("mjo") },
            R { pts: 40, sig: Sig::ExeContains("majiro") },
            R { pts: 20, sig: Sig::Ext("arc") },
        ],
    },
    Rec {
        id: "yuris",
        name: "YU-RIS",
        priority: 33,
        advice: "YU-RIS 系。封包为 .ypf（可带密码），立绘 .ybn，脚本 .yui/.yus。\
                 可用 GARbro（需选对密钥/引擎版本）解包。",
        rules: &[
            R { pts: 65, sig: Sig::Ext("ypf") },
            R { pts: 20, sig: Sig::Ext("ybn") },
            R { pts: 15, sig: Sig::Ext("yui") },
        ],
    },
    Rec {
        id: "catsystem2",
        name: "CatSystem2",
        priority: 32,
        advice: "CatSystem2 系。脚本 .cst 编译在 .int 里；可用 GARbro 解包，\
                 文本在 .cst/.int 中。",
        rules: &[
            R { pts: 45, sig: Sig::Ext("int") },
            R { pts: 25, sig: Sig::Ext("cst") },
            R { pts: 15, sig: Sig::Ext("noa") },
        ],
    },
    Rec {
        id: "alicesoft",
        name: "AliceSoft System（.ald）",
        priority: 34,
        advice: "AliceSoft 系（《兰斯》《战兰》等）。封包 .ald/.afa，可用 GARbro 解包；\
                 文本在 .ain/.dat 脚本里。",
        rules: &[
            R { pts: 65, sig: Sig::Ext("ald") },
            R { pts: 40, sig: Sig::Ext("afa") },
            R { pts: 20, sig: Sig::Ext("alk") },
        ],
    },
    Rec {
        id: "livemaker",
        name: "LiveMaker",
        priority: 31,
        advice: "LiveMaker。资源 .prt/.grp/.grs，可用 GARbro 解包；\
                 剧情在 .lsc/.lse 脚本里。",
        rules: &[
            R { pts: 55, sig: Sig::Ext("prt") },
            R { pts: 30, sig: Sig::Ext("grp") },
            R { pts: 15, sig: Sig::Ext("grs") },
        ],
    },
    Rec {
        id: "nitroplus",
        name: "Nitroplus（.npk / .npa）",
        priority: 31,
        advice: "Nitroplus 系（《沙耶之歌》《君与彼女》等）。封包 .npk/.npa，\
                 可用 GARbro 解包；脚本为 .scr。",
        rules: &[
            R { pts: 65, sig: Sig::ExtAny(&["npk", "npa"]) },
            R { pts: 20, sig: Sig::Ext("scr") },
        ],
    },
    Rec {
        id: "rpgmaker2k",
        name: "RPG Maker 2000 / 2003",
        priority: 46,
        advice: "RPG Maker 2000/2003（不是 RGSS！）。数据库 RPG_RT.ldb、地图 .lmt，\
                 文本是加密存储的，建议用 GARbro 或专用工具（如 ldb 编辑器）处理；\
                 本工具未内置该格式。",
        rules: &[
            R { pts: 60, sig: Sig::Ext("ldb") },
            R { pts: 30, sig: Sig::ExtN("lmt", 1) },
            R { pts: 30, sig: Sig::ExeContains("rpg_rt") },
        ],
    },
    Rec {
        id: "unreal",
        name: "Unreal Engine",
        priority: 40,
        advice: "Unreal Engine。资源 .uasset 打在 .pak（UE4）或 .utoc/.ucas（UE5 IoStore），\
                 需用 FModel/UnrealPak 处理；汉化通常走本地化（int/csv）+ pak 回填。\
                 解锁：SaveGames/*.sav 是蓝图序列化二进制，难度高，优先走自带存档替换。",
        rules: &[
            R { pts: 65, sig: Sig::ExtAny(&["utoc", "ucas"]) },
            R { pts: 45, sig: Sig::Ext("uasset") },
            R { pts: 45, sig: Sig::RootDir("engine") },
            R { pts: 30, sig: Sig::Dir("binaries") },
            R { pts: 15, sig: Sig::Dir("content") },
        ],
    },
    Rec {
        id: "love2d",
        name: "LÖVE（Love2D）",
        priority: 30,
        advice: "LÖVE 游戏。.love 本质是 zip，可直接改后缀解压；\
                 文本通常在 lua 脚本或 .json 里，改完重新打包成 .love/.exe。",
        rules: &[
            R { pts: 65, sig: Sig::Ext("love") },
            R { pts: 30, sig: Sig::RootFile("main.lua") },
            R { pts: 15, sig: Sig::Ext("lua") },
        ],
    },
    Rec {
        id: "gamemaker",
        name: "GameMaker Studio",
        priority: 30,
        advice: "GameMaker 系。data.win 是编译后的游戏数据（含全部字符串），\
                 可用 UndertaleModTool 替换字符串；本工具未内置该格式。",
        rules: &[
            R { pts: 65, sig: Sig::RootFile("data.win") },
            R { pts: 15, sig: Sig::Ext("win") },
        ],
    },
    Rec {
        id: "purple",
        name: "Purple Software（.lpk）",
        priority: 28,
        advice: "疑似 Purple 系。封包 .lpk，可用 GARbro 尝试解包；\
                 若解不开说明带校验，建议换专用工具。",
        rules: &[R { pts: 60, sig: Sig::Ext("lpk") }],
    },
    Rec {
        id: "cocos2dx",
        name: "Cocos2d-x / JSB",
        priority: 30,
        advice: "Cocos2d-x / JSB（多见于 DLsite 日系小游戏）。脚本与资源在 Resources/\
                 （script/jsb*.js、data/）；解锁可 grep jsb*.js 的解锁变量，\
                 或改 Resources/ 或用户目录下的 JSON/csv 明文存档。",
        rules: &[
            R { pts: 65, sig: Sig::FileNamed("libcocos2d.dll") },
            R { pts: 20, sig: Sig::RootDir("resources") },
            R { pts: 15, sig: Sig::Dir("script") },
        ],
    },
    Rec {
        id: "flash",
        name: "Flash / SWF",
        priority: 30,
        advice: "Flash / SWF。用 JPEXS FFDec 反编译 ActionScript、导出资源并可回写；\
                 存档是 SharedObject（%APPDATA%/Macromedia/Flash Player/#SharedObjects/...*.sol），\
                 可用 .sol 编辑器改数值解锁。",
        rules: &[
            R { pts: 60, sig: Sig::Ext("swf") },
            R { pts: 20, sig: Sig::ExeAnyContains(&["flash", "adobe"]) },
        ],
    },
    Rec {
        id: "qsp",
        name: "QSP（Quest Soft Player）",
        priority: 30,
        advice: "QSP。`.qsps` 是明文文本源码，可直接编辑条件判断 / 变量初始化；\
                 `.qsp` 为容器，用 QSP 官方工具或文本提取器解开。",
        rules: &[
            R { pts: 60, sig: Sig::ExtAny(&["qsps", "qsp"]) },
            R { pts: 20, sig: Sig::ExeAnyContains(&["qsp"]) },
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_rec_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn detect(id: &str, d: &Path) -> Detection {
        let scan = ScanCtx::build(d);
        let rec = RECOGNIZERS.iter().find(|r| r.id == id).unwrap();
        Recognizer(rec).detect_scan(&scan)
    }

    #[test]
    fn test_bgi_requires_distinctive_signal() {
        // 只有 .arc：弱特征，不应过线（避免把别的引擎误判成 BGI）
        let d = dir("bgi_weak");
        std::fs::write(d.join("data.arc"), b"x").unwrap();
        let det = detect("bgi_ethornell", &d);
        assert_eq!(det.score, 40);
        assert!(!det.ok(), "仅 .arc 不该判定为 Ethornell");
        // 加上 BGI.exe → 过线
        std::fs::write(d.join("BGI.exe"), b"x").unwrap();
        let det = detect("bgi_ethornell", &d);
        assert!(det.ok(), "BGI.exe + .arc 应判定：{}", det.score);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_siglus_needs_two_signals() {
        let d = dir("siglus");
        std::fs::write(d.join("Gameexe.dat"), b"x").unwrap();
        assert!(!detect("siglus", &d).ok(), "只有 Gameexe.dat 应为疑似");
        std::fs::write(d.join("Scene.pak"), b"x").unwrap();
        assert!(detect("siglus", &d).ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_strong_extension_alone_passes() {
        for (id, file) in [
            ("majiro", "script.mjo"),
            ("yuris", "data.ypf"),
            ("alicesoft", "data.ald"),
            ("nitroplus", "data.npk"),
            ("gamemaker", "data.win"),
            ("love2d", "game.love"),
            ("unreal", "global.utoc"),
        ] {
            let d = dir(id);
            std::fs::write(d.join(file), b"x").unwrap();
            let det = detect(id, &d);
            assert!(det.ok(), "{id} 的强特征 {file} 应单独过线，实得 {} 分", det.score);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn test_rpgmaker2k_not_confused_with_rgss() {
        let d = dir("rm2k");
        std::fs::create_dir_all(d.join("Data")).unwrap();
        std::fs::write(d.join("RPG_RT.ldb"), b"x").unwrap();
        std::fs::write(d.join("RPG_RT.lmt"), b"x").unwrap();
        std::fs::write(d.join("RPG_RT.exe"), b"x").unwrap();
        let det = detect("rpgmaker2k", &d);
        assert!(det.ok(), "RPG_RT.* 应判定为 2000/2003");
        // 不应把 RGSS 的 Data/ 目录算进来（本识别器没有该判据）
        assert!(!det.evidence.iter().any(|e| e.contains("Graphics")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_empty_dir_no_evidence() {
        let d = dir("empty");
        for rec in RECOGNIZERS {
            let det = detect(rec.id, &d);
            assert_eq!(det.score, 0, "空目录不该给 {} 任何分", rec.id);
            assert!(det.evidence.is_empty());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_recognizer_priority_below_real_engines() {
        for r in RECOGNIZERS {
            assert!(r.priority < 60, "识别器 {} 优先级过高，可能抢占正式引擎", r.id);
        }
        // 判定线为 60 由 const assert 在编译期保证（见 LINE 定义处）
    }

    #[test]
    fn test_bgi_via_notable_files() {
        // BREGEXP.DLL 单独即为强特征（实测 ωstar《美少女万華鏡》）
        let d = dir("bgi_notable");
        std::fs::write(d.join("BREGEXP.DLL"), b"x").unwrap();
        assert!(detect("bgi_ethornell", &d).ok(), "BREGEXP.DLL 应单独过线");
        let _ = std::fs::remove_dir_all(&d);

        // bgi.gdb 稍弱（55），配 .arc 即过线
        let d = dir("bgi_gdb");
        std::fs::write(d.join("bgi.gdb"), b"x").unwrap();
        assert!(!detect("bgi_ethornell", &d).ok(), "bgi.gdb 单独仅为疑似");
        std::fs::write(d.join("data.arc"), b"x").unwrap();
        assert!(detect("bgi_ethornell", &d).ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    // Artemis 已升级为**原生插件**（`engines/artemis.rs`，可解包+回封），
    // 因此不再由表驱动识别器覆盖；其检测/解包断言见 artemis.rs 的测试。

    #[test]
    fn test_new_recognizers_cocos_flash_qsp() {
        for (id, files) in [
            ("cocos2dx", vec!["libcocos2d.dll"]),
            ("flash", vec!["game.swf"]),
            ("qsp", vec!["story.qsps"]),
        ] {
            let d = dir(id);
            for f in &files {
                std::fs::write(d.join(f), b"x").unwrap();
            }
            let det = detect(id, &d);
            assert!(det.ok(), "{id} 应能识别，实得 {} 分", det.score);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn test_unreal_by_directory_layout() {
        let d = dir("unreal_layout");
        std::fs::create_dir_all(d.join("Engine")).unwrap();
        assert!(!detect("unreal", &d).ok(), "只有 Engine/ 应为疑似");
        std::fs::create_dir_all(d.join("MyGame").join("Binaries")).unwrap();
        assert!(detect("unreal", &d).ok(), "Engine/ + Binaries/ 应判定");
        let _ = std::fs::remove_dir_all(&d);
    }
}
