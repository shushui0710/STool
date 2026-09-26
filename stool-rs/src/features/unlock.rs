//! 统一「全 CG / 画廊解锁」抽象层。
//!
//! 背景：此前只有 Ren'Py（pickle `persistent`）与 Unity（PlayerPrefs 注册表）各自实现了
//! `Op::Unlock`，其余 8 个原生插件 + 15 个盲区识别器**完全没有解锁入口**，"全 CG 解锁"
//! 实际只覆盖 2 个引擎。而实测盘点（见 `docs/引擎识别依据与解锁策略.md`）显示：
//! 网盘分享包常年附带 `全CG存档/`，这条"替换存档"路线**与引擎无关、成功率最高**。
//!
//! 本模块把"识别到什么引擎 → 用哪种手段解锁"从各插件里抽出来，变成
//! **一张声明式策略表（[`UNLOCK_CATALOG`]）+ 一个统一调度器（[`run`]）**：
//!
//! ```text
//!                     ┌──────────────  UnlockSpec（按 engine_id 查表）
//!   Registry / CLI ─▶ │ routes[]     手段（按引擎偏好排序）
//!   Engine::unlock()  │ save_dirs[]  存档目录候选
//!                     │ basis/action 识别依据 + 解锁动作（供 UI / 文档）
//!                     └───────┬───────
//!                             ▼
//!                     run(engine_id, ctx, engine_impl)
//!                             │  pick_route
//!        ┌──────────┬─────────┼──────────┬───────────┬──────────┐
//!        ▼          ▼         ▼          ▼           ▼          ▼
//!   BundledSave  Registry  SaveFileFlag ConfigFlag InGameSwitch Manual
//!    (本模块通用) (Unity)   (Ren'Py)    (通用指引)  (通用指引)   (通用指引)
//! ```
//!
//! 安全默认与 `gallery::unlock` 完全一致：**缺省只读预览**，`--opt:apply=1` 才落地；
//! 任何覆盖写盘前一律 `settings::backup_once()`（备份已存在即不覆盖），可用
//! `stool restore` 还原。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::engines::{Ctx, OpOutcome};

// ---------------------------------------------------------------------------
// 手段分类
// ---------------------------------------------------------------------------

/// 解锁手段。按"稳妥 / 通用程度"从高到低排列（[`UnlockRoute::order`] 越小越优先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UnlockRoute {
    /// 游戏目录自带「全CG存档」：按同名复制进存档目录即可。
    /// 与引擎无关、零风险，成功率最高 —— 默认路线。
    BundledSave,
    /// 游戏内自带的隐藏全开开关（Debug / 设置项）。无法自动，只能提示去找。
    InGameSwitch,
    /// 明文 / 半明文配置文件里的解锁开关（json / ini / xml / txt）。
    ConfigFlag,
    /// 注册表键位（Unity PlayerPrefs 等）。
    Registry,
    /// 存档文件里的位标志 / 计数（二进制存档、pickle persistent）。
    SaveFileFlag,
    /// 无已知自动化手段，需人工分析（Cheat Engine / 内存 hook）。
    Manual,
}

impl UnlockRoute {
    /// 全部手段（固定顺序，供 GUI 下拉 / 文档）。
    pub fn all() -> &'static [UnlockRoute] {
        &[
            UnlockRoute::BundledSave,
            UnlockRoute::InGameSwitch,
            UnlockRoute::ConfigFlag,
            UnlockRoute::Registry,
            UnlockRoute::SaveFileFlag,
            UnlockRoute::Manual,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            UnlockRoute::BundledSave => "替换自带全CG存档",
            UnlockRoute::InGameSwitch => "游戏内全开开关",
            UnlockRoute::ConfigFlag => "改配置文件开关",
            UnlockRoute::Registry => "改注册表键位",
            UnlockRoute::SaveFileFlag => "改存档位 / 计数",
            UnlockRoute::Manual => "人工分析",
        }
    }

    pub fn desc(&self) -> &'static str {
        match self {
            UnlockRoute::BundledSave => {
                "把网盘包附带的《全CG存档》按同名复制进存档目录（不动游戏文件，与引擎无关）"
            }
            UnlockRoute::InGameSwitch => {
                "不少作品在设置 / 隐藏界面留了 toggle，打开即全解锁（需人工找）"
            }
            UnlockRoute::ConfigFlag => "改 json / ini / xml 等配置文件里的解锁开关",
            UnlockRoute::Registry => "写注册表键位（如 Unity PlayerPrefs 的 <键>_h<djb2>）",
            UnlockRoute::SaveFileFlag => "改二进制 / pickle 存档里的位标志或计数",
            UnlockRoute::Manual => "无已知自动化手段，需 Cheat Engine / 内存 hook 等人工处理",
        }
    }

    /// 是否有内置自动实现（false = 只能给指引）。
    pub fn automated(&self) -> bool {
        matches!(
            self,
            UnlockRoute::BundledSave
                | UnlockRoute::ConfigFlag
                | UnlockRoute::Registry
                | UnlockRoute::SaveFileFlag
        )
    }

    /// 默认路线选择顺序：越小越优先（与 skill 的「通用解锁路线」一致）。
    pub fn order(&self) -> u8 {
        match self {
            UnlockRoute::BundledSave => 0,
            UnlockRoute::InGameSwitch => 1,
            UnlockRoute::ConfigFlag => 2,
            UnlockRoute::Registry => 3,
            UnlockRoute::SaveFileFlag => 4,
            UnlockRoute::Manual => 5,
        }
    }

    /// `--opt:route=` 的取值。
    pub fn key(&self) -> &'static str {
        match self {
            UnlockRoute::BundledSave => "bundled",
            UnlockRoute::InGameSwitch => "ingame",
            UnlockRoute::ConfigFlag => "config",
            UnlockRoute::Registry => "registry",
            UnlockRoute::SaveFileFlag => "savefile",
            UnlockRoute::Manual => "manual",
        }
    }

    /// 解析 `--opt:route=` 取值（含常用别名）。
    pub fn from_key(k: &str) -> Option<Self> {
        Some(match k.trim().to_ascii_lowercase().as_str() {
            "bundled" | "bundle" | "save" | "存档" => UnlockRoute::BundledSave,
            "ingame" | "in-game" | "switch" | "toggle" => UnlockRoute::InGameSwitch,
            "config" | "cfg" | "ini" | "json" => UnlockRoute::ConfigFlag,
            "registry" | "reg" | "prefs" | "playerprefs" => UnlockRoute::Registry,
            "savefile" | "save-file" | "flag" | "persistent" => UnlockRoute::SaveFileFlag,
            "manual" | "none" => UnlockRoute::Manual,
            _ => return None,
        })
    }
}

/// 某引擎的解锁策略声明（与 `recognize.rs` 的识别规则互补：那边讲"怎么认"，这边讲"怎么解"）。
#[derive(Debug, Clone, Copy)]
pub struct UnlockSpec {
    pub engine_id: &'static str,
    /// 支持的手段（顺序 = 该引擎的偏好顺序；`BundledSave` 只要存在就会被优先选中）。
    pub routes: &'static [UnlockRoute],
    /// 存档目录候选（相对游戏根；`"."` 表示游戏根目录本身）。
    pub save_dirs: &'static [&'static str],
    /// 识别依据（一句话，与检测判据对应）。
    pub basis: &'static str,
    /// 解锁动作（一句话）。
    pub action: &'static str,
    /// 附加提示。
    pub note: &'static str,
}

// ---------------------------------------------------------------------------
// 策略表
// ---------------------------------------------------------------------------

use UnlockRoute as R;

/// 兜底策略（未知引擎 / 兜底插件）：任何游戏都至少能试"替换存档 + 人工"这两条。
static GENERIC_SPEC: UnlockSpec = UnlockSpec {
    engine_id: "generic",
    routes: &[
        R::BundledSave,
        R::InGameSwitch,
        R::ConfigFlag,
        R::SaveFileFlag,
        R::Manual,
    ],
    save_dirs: &[".", "savedata", "save", "SaveData", "saves"],
    basis: "未命中任何已收录引擎特征",
    action: "先试替换自带全CG存档；再找游戏内全开开关；最后 Cheat Engine / 内存 hook",
    note: "先用 detect 确认目录层级，并配置 GARbro 尝试解包",
};

/// 引擎 id → 解锁策略。**新增引擎时只需在这里加一行**（以及 recognize.rs 的识别规则）。
pub static UNLOCK_CATALOG: &[UnlockSpec] = &[
    // ---------------- 原生插件 ----------------
    UnlockSpec {
        engine_id: "renpy",
        routes: &[R::SaveFileFlag, R::BundledSave, R::InGameSwitch],
        save_dirs: &["game/saves", "saves", "game"],
        basis: "renpy/ + game/ 目录，含 .rpa / .rpyc",
        action: "改 persistent（pickle）里 unlock/seen/clear/cg/gallery 布尔为 True",
        note: "存档在 %AppData%/RenPy/<游戏名>/；建议先跑一次游戏生成 persistent",
    },
    UnlockSpec {
        engine_id: "rpgmaker_mv",
        routes: &[R::BundledSave, R::ConfigFlag, R::InGameSwitch],
        save_dirs: &["www/save", "save", "www/saves", "www"],
        basis: "data/Actors.json + js/rpg_core.js（或 game.rpgproject）",
        action: "替换 www/save/ 下 Save*.rpgsave；或改 js 插件里的回廊开关",
        note: "存档是明文 JSON（.rpgsave），可直接搜 unlock/gallery 字段",
    },
    UnlockSpec {
        engine_id: "rpgmaker_rgss",
        routes: &[R::BundledSave, R::SaveFileFlag, R::ConfigFlag],
        save_dirs: &["Save", "save", "Saves"],
        basis: "Data/Scripts.rvdata2 + Game.ini（XP/VX/VX Ace）",
        action: "替换 Save*.rvdata2；或改 Game.ini / Scripts 里的回廊开关",
        note: "rvdata2 为 Ruby marshal 序列化，本工具存档页可解析编辑",
    },
    UnlockSpec {
        engine_id: "kirikiri",
        routes: &[R::BundledSave, R::ConfigFlag, R::InGameSwitch],
        save_dirs: &["savedata", "save", "."],
        basis: "*.xp3（data.xp3 等）+ .ks / .tjs 脚本",
        action: "改 savedata/*.dat 里的全局 flag；或解 xp3 后改 .ks 判定",
        note: "KAG 剧本为明文，可 grep clear/seen/cg/album",
    },
    UnlockSpec {
        engine_id: "godot",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &[".", "saves", "user"],
        basis: "project.godot 或 *.pck",
        action: "改 user:// 存档（%APPDATA%/Godot/app_userdata/<游戏名>/，多为 JSON）",
        note: "解 pck 后 .gd 源码通常明文，直接 grep unlock/gallery 变量",
    },
    UnlockSpec {
        engine_id: "nscripter",
        routes: &[R::BundledSave, R::ConfigFlag, R::InGameSwitch],
        save_dirs: &["savedata", "save", "."],
        basis: "nscript.dat + arc.nsa / *.nsa",
        action: "改 savedata 里的全局变量（cif）；或解 nscript.dat 改判定",
        note: "nscript.dat 为循环 XOR 文本，可还原后 grep",
    },
    UnlockSpec {
        engine_id: "tyrano",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "tyrano/tyrano.js + .tjs",
        action: "改 savedata 里的 JSON；或改 .ks / .tjs 判定",
        note: "",
    },
    UnlockSpec {
        engine_id: "html_game",
        routes: &[R::BundledSave, R::ConfigFlag, R::InGameSwitch],
        save_dirs: &[".", "resources/app", "save"],
        basis: "index.html + js/（Electron 版有 resources/app.asar）",
        action: "改 localStorage / IndexedDB 或 JSON 存档；Electron 版可解 asar 后改 JS",
        note: "F12 控制台可直接操作全局对象",
    },
    UnlockSpec {
        engine_id: "wolf",
        routes: &[R::BundledSave, R::SaveFileFlag],
        save_dirs: &["SaveData", "save", "."],
        basis: "Data/BasicData + *.wolf 封包",
        action: "替换 SaveData/SaveData.dat；或改 Data 地图事件里的开关",
        note: "WolfDec 解包后地图事件为明文 CSV",
    },
    UnlockSpec {
        engine_id: "unity",
        routes: &[R::Registry, R::BundledSave, R::InGameSwitch],
        save_dirs: &[".", "saves", "SaveData"],
        basis: "UnityPlayer.dll + <游戏>_Data/app.info（Mono / IL2CPP 通用）",
        action: "写 PlayerPrefs 注册表 HKCU\\Software\\<company>\\<product> 的 <键>_h<djb2>=1",
        note: "不动游戏文件、无完整性校验风险；很多作品自带 Gallery Open 开关更彻底",
    },
    // ---------------- 盲区识别器 ----------------
    UnlockSpec {
        engine_id: "bgi_ethornell",
        routes: &[R::BundledSave, R::SaveFileFlag, R::ConfigFlag, R::InGameSwitch],
        save_dirs: &["SaveData", "savedata", "save", "."],
        basis: "BREGEXP.DLL / bgi.gdb / エンジン設定.exe + *.arc 或 GameData/*.pack",
        action: "替换 SaveData/SystemData.dat（网盘包常附）；或改 SystemData 回廊标记",
        note: "ωstar《美少女万華鏡》系列即此布局",
    },
    UnlockSpec {
        engine_id: "siglus",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "SaveData", "."],
        basis: "Gameexe.dat + *.pak",
        action: "替换 savedata；或解 pak 后改 Scene 脚本判定",
        note: "Key 系《CLANNAD》《Summer Pockets》等",
    },
    UnlockSpec {
        engine_id: "reallive",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "Gameexe.ini + seen.txt + *.g00",
        action: "改 savedata 与 seen.txt（已读表）",
        note: "早期 Key 作品（Kanon / AIR）",
    },
    UnlockSpec {
        engine_id: "majiro",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.mjo（脚本）+ exe 名含 majiro",
        action: "改 savedata 里的全局 flag",
        note: "",
    },
    UnlockSpec {
        engine_id: "yuris",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.ypf（封包）+ .ybn / .yui",
        action: "改 savedata/*.dat 的 clear flag",
        note: "ypf 可能带密码，需 GARbro 选对引擎版本",
    },
    UnlockSpec {
        engine_id: "catsystem2",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.int（脚本编译体）+ .cst",
        action: "改 savedata/*.sav（CIF 系存档）",
        note: "",
    },
    UnlockSpec {
        engine_id: "artemis",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.pfs 且带分卷（x.pfs.000/001）/ .pfs2 / .asb",
        action: "改 savedata/*.sav",
        note: "注意：单个 .pfs 与 BGI 同名，需有分卷才是 Artemis",
    },
    UnlockSpec {
        engine_id: "alicesoft",
        routes: &[R::BundledSave, R::SaveFileFlag],
        save_dirs: &["SaveData", "savedata", "."],
        basis: "*.ald / *.afa（+ system40.exe）",
        action: "改 SaveData 里的回廊 flag（AliceSoft 存档含 CG 计数）",
        note: "",
    },
    UnlockSpec {
        engine_id: "livemaker",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.prt / .grp / .grs",
        action: "改 savedata/*.sav",
        note: "",
    },
    UnlockSpec {
        engine_id: "nitroplus",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.npk / *.npa + .scr",
        action: "解 npk 后改 .scr 判定；或改 savedata",
        note: "",
    },
    UnlockSpec {
        engine_id: "rpgmaker2k",
        routes: &[R::BundledSave, R::SaveFileFlag],
        save_dirs: &["Save", "save", "."],
        basis: "RPG_RT.ldb + *.lmt（2000/2003，非 RGSS）",
        action: "替换 Save*.lsd；或改数据库开关",
        note: "lsd 为私有二进制，需专用编辑器",
    },
    UnlockSpec {
        engine_id: "unreal",
        routes: &[R::BundledSave, R::SaveFileFlag, R::InGameSwitch],
        save_dirs: &[".", "Saved", "SaveGames"],
        basis: "Engine/ + Binaries/ + Content/ 布局，*.utoc/*.ucas/*.uasset",
        action: "替换 Saved/SaveGames/*.sav（蓝图序列化，难度最高）",
        note: "优先走自带存档 / 内存路线",
    },
    UnlockSpec {
        engine_id: "love2d",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &[".", "save", "saves"],
        basis: "*.love（本质 zip）或根目录 main.lua",
        action: "解 .love 改 lua；或改 save 目录 JSON",
        note: "",
    },
    UnlockSpec {
        engine_id: "gamemaker",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &[".", "save", "saves"],
        basis: "根目录 data.win（编译后游戏数据）",
        action: "用 UndertaleModTool 改 data.win 字符串 / 变量；或改存档目录",
        note: "存档常在 %LOCALAPPDATA%/<游戏名>/",
    },
    UnlockSpec {
        engine_id: "purple",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &["savedata", "save", "."],
        basis: "*.lpk（疑似 Purple 系）",
        action: "改 savedata",
        note: "lpk 若带校验可能解不开，建议换专用工具",
    },
    // ---------------- 本次新增识别器 ----------------
    UnlockSpec {
        engine_id: "cocos2dx",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &[".", "Resources", "save"],
        basis: "libcocos2d.dll + Resources/（内含 script/jsb*.js、data/）",
        action: "改 Resources/script/jsb*.js 判定；或改 JSON / csv 明文存档",
        note: "多见于 DLsite 日系小游戏",
    },
    UnlockSpec {
        engine_id: "flash",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &[".", "save"],
        basis: "*.swf 或套壳 exe",
        action: "改 .sol 存档（%APPDATA%/Macromedia/Flash Player/...）；或用 FFDec 改 ActionScript",
        note: "FFDec 可反编译并回写 SWF",
    },
    UnlockSpec {
        engine_id: "qsp",
        routes: &[R::BundledSave, R::ConfigFlag],
        save_dirs: &[".", "save"],
        basis: "*.qsp / *.qsps + qsp 播放器",
        action: "改 .qsps 明文源码里的条件判断 / 变量初始化",
        note: ".qsps 是明文文本，直接编辑",
    },
];

/// 查策略；未收录返回 `None`。
pub fn spec_for(engine_id: &str) -> Option<&'static UnlockSpec> {
    UNLOCK_CATALOG.iter().find(|s| s.engine_id == engine_id)
}

/// 查策略（未收录则返回兜底策略）。
pub fn spec_or_generic(engine_id: &str) -> &'static UnlockSpec {
    spec_for(engine_id).unwrap_or(&GENERIC_SPEC)
}

/// 该引擎支持的解锁手段（未收录返回空 —— 但兜底策略实际覆盖全部未知引擎）。
pub fn routes_for(engine_id: &str) -> &'static [UnlockRoute] {
    spec_for(engine_id).map(|s| s.routes).unwrap_or(GENERIC_SPEC.routes)
}

/// `--opt:route=` 的合法取值提示。
pub fn route_keys() -> String {
    UnlockRoute::all().iter().map(|r| r.key()).collect::<Vec<_>>().join(" | ")
}

// ---------------------------------------------------------------------------
// 存档目录 / 自带存档定位（引擎无关）
// ---------------------------------------------------------------------------

/// 常见存档目录名关键字（小写包含匹配）。
const SAVE_DIR_KEYWORDS: &[&str] = &["save", "存档", "セーブ", "ユーザーデータ", "userdata"];

/// 「自带全CG存档」目录名关键字 —— 刻意保守，避免把游戏自己的 gallery 素材目录当存档。
const BUNDLE_DIR_KEYWORDS: &[&str] = &[
    "全cg",
    "cg存档",
    "全开",
    "全解锁",
    "完全存档",
    "100%",
    "回想",
    "回廊",
    "セーブデータ",
];

/// `<任意名>存档` 这种泛化命名（`C0771存档`、`某作存档`、`data 存档`…）。
///
/// 为什么单列一档：分享包里的存档目录常带**作者/编号前缀**（`C0771存档` 就是
/// 「C0771」这个同人社团编号 + `存档`），`BUNDLE_DIR_KEYWORDS` 里的 `全cg`/`全开`
/// 之类**一个都匹配不上** → 检测不到、反而被 `SAVE_DIR_KEYWORDS` 当成目标目录。
///
/// **只认「后缀」**：目录名以「存档」结尾才算，避免 `存档备份工具` 这种误伤。
fn is_archive_dir_name(name_lower: &str) -> bool {
    name_lower.ends_with("存档") && name_lower.chars().count() >= 2
}

/// 目录名**明确不能**当「自带存档来源」的（命中即否决上面的泛化规则）。
///
/// `存档` 两个字太泛，必须给几条硬否决，否则游戏的资产/日志目录会被卷进来：
/// - `save`/`savedata` 之类**是游戏的运行期存档目录**，不是分享包里的存档包；
/// - `backup`/`备份` 是备份，不是「装进去就全开」的存档；
/// - 明显是素材/资源的目录。
const NOT_BUNDLE_DIR: &[&str] = &["backup", "备份", "素材", "resource", "assets", "log", "cache"];

/// 「自带全CG存档」文件名关键字。
const BUNDLE_FILE_KEYWORDS: &[&str] = &[
    "全cg",
    "全开",
    "全解锁",
    "clear",
    "gallery",
    "complete",
    "100%",
    "回想",
    "回廊",
];

/// 存档专属扩展名（大写小写都按小写比）。
///
/// 这些扩展名**基本只用于存档**，命中即可信。
const SAVE_EXTS: &[&str] = &[
    "sav", "save", "sav2", "rpgsave", "rmmzsave", "rmzsave", "rvdata2", "rvdata", "lsd", "sol",
    "cif", "gdb", "sgd",
];

/// 通用扩展名 —— 引擎自己也大量使用，**光看扩展名不足以判定是存档**。
///
/// `dat`/`bin`/`json` 这类必须**配合存档语义的文件名**（见 [`looks_like_save_name`]）才算。
/// 起因：实测某游戏根目录里 `unins000.dat`、`settings.json`、`catalog.json`、
/// `ScriptingAssemblies.json` 全都命中过旧的 `SAVE_EXTS` —— 一旦分享包把这些带进
/// 存档目录，就会被当成存档复制过去。
const GENERIC_EXTS: &[&str] = &["dat", "bin", "json", "sd", "save"];

/// 文件名里出现这些词，才认为通用扩展名的文件「像存档」。
const SAVE_NAME_HINTS: &[&str] = &[
    "save", "savedata", "存档", "セーブ", "system", "global", "data", "config", "user",
];

fn name_matches(name_lower: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|k| name_lower.contains(k))
}

/// **唯一判据**：这个目录名是不是「自带存档来源」。
///
/// `find_bundled_saves`（找来源）与 `find_save_dirs`（排目标）**必须共用它** ——
/// 否则就会出现「同一个目录既当来源又当目标 → 复制到自身」那种自相矛盾。
pub fn is_bundle_dir_name(name_lower: &str) -> bool {
    if name_matches(name_lower, NOT_BUNDLE_DIR) {
        return false;
    }
    name_matches(name_lower, BUNDLE_DIR_KEYWORDS) || is_archive_dir_name(name_lower)
}

/// 从「存档说明文件」里解析出目标存档目录。
///
/// 分享包常在存档目录里塞一个 `位置.txt` / `说明.txt` / `readme.txt`，里面写着
/// 存档该放哪，例如：
///
/// ```text
/// C:\Users\（用户名）\AppData\LocalLow\GIRL'S SOFTWARE\KKC4
/// ```
///
/// 这条线索**比任何目录名启发式都准**（是打包者亲手写的），所以优先采信。
/// 只认「像绝对路径」的行，并把 `（用户名）` / `<用户名>` 之类的占位换成当前用户。
///
/// 返回 `(目录PathBuf, 原始行)`；解析不出返回 `None`。
pub fn parse_save_location(text: &str) -> Option<(PathBuf, String)> {
    for raw in text.lines() {
        let line = raw.trim().trim_matches(|c| c == '"' || c == '\u{feff}');
        if line.len() < 4 {
            continue;
        }
        // 盘符开头（C:\ / D:/）或 UNC（\\server\share）才认，别把句中文说明当路径。
        let looks_abs = line
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
            && line.get(1..3).map(|s| s == ":\\" || s == ":/").unwrap_or(false);
        if !looks_abs {
            continue;
        }
        // 占位符 → 当前用户名（打包者不可能知道买家的用户名）。
        let expanded = expand_user_placeholder(line);
        return Some((PathBuf::from(expanded), line.to_string()));
    }
    None
}

/// 把 `（用户名）` / `<用户名>` / `%USERNAME%` / `{用户名}` 换成真实用户名。
fn expand_user_placeholder(line: &str) -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() {
        return line.to_string();
    }
    let mut s = line.to_string();
    for pat in [
        "（用户名）",
        "(用户名)",
        "<用户名>",
        "{用户名}",
        "%USERNAME%",
        "（username）",
        "(username)",
        "<username>",
    ] {
        if s.contains(pat) {
            s = s.replace(pat, &user);
        }
    }
    s
}

/// 候选的「存档说明文件名」（小写精确匹配，避免把游戏文案当说明读）。
const LOCATION_FILE_NAMES: &[&str] = &["位置.txt", "存档位置.txt", "说明.txt", "readme.txt", "路径.txt"];

/// 在某个目录（及浅层子目录）里找 `位置.txt` 之类的说明文件，解析出目标存档目录。
fn find_declared_save_dir(dir: &Path) -> Option<(PathBuf, String)> {
    /// 读一个候选文件；名字不在白名单、或解析不出路径 → `None`。
    fn read(p: &Path) -> Option<(PathBuf, String)> {
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        // 说明文件常带 BOM / 是 GBK；这里优先 UTF-8，失败再按 GBK 试。
        if !LOCATION_FILE_NAMES.iter().any(|n| name == n.to_lowercase()) {
            return None;
        }
        let bytes = fs::read(p).ok()?;
        let text = decode_text(&bytes)?;
        parse_save_location(&text)
    }
    // 先看自己这一层（`C0771存档/C0771存档/存档/位置.txt` 的最内层就是）。
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                if let Some(hit) = read(p.as_path()) {
                    return Some(hit);
                }
            }
        }
    }
    // 再往浅层子目录里找（说明常和存档同放一个子目录）。
    //
    // 深度必须够：实录 `C0771存档/C0771存档/存档/位置.txt` 在 **depth=4**，
    // 早先写死 `max_depth(3)` 就会「解析函数明明是对的、却永远看不到那个文件」。
    // 这里与 `find_bundled_saves` 统一用 8，免得两边不一致又踩一次。
    for e in WalkDir::new(dir).max_depth(8).follow_links(false).into_iter().flatten() {
        if !e.file_type().is_file() {
            continue;
        }
        if let Some(hit) = read(e.path()) {
            return Some(hit);
        }
    }
    None
}

/// 把一段文本按 UTF-8 → GBK 顺序解码（Windows 上的说明文件常是 GBK）。
///
/// ⚠️ 别拿 `decode()` 的 `ok` 标志当「解码失败」：`encoding_rs` 对**只要有一个字节
/// 无法映射**就回 `ok=false`，可其余字节其实解得好好的。实测 `位置.txt` 里的
/// `（用户名）` 是 GBK 的 `A3 A8 …`，UTF-8 解不了 → 走 GBK 分支 → `ok=false`
/// → 若据此退回 `from_utf8_lossy`，汉字全变成 U+FFFD，
/// `（用户名）` 就匹配不上、路径作废、目标目录凭空消失。
///
/// 正确做法：**GBK 解码结果照用**（它已经用替换字符标出了真正读不懂的位置）。
fn decode_text(bytes: &[u8]) -> Option<String> {
    // 去 BOM
    let b = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { &bytes[3..] } else { bytes };
    if let Ok(s) = std::str::from_utf8(b) {
        return Some(s.to_string());
    }
    // 不是合法 UTF-8 → 按 GBK 解。中文 Windows 上的 .txt 绝大多数就是这个。
    let (cow, _, _) = encoding_rs::GBK.decode(b);
    Some(cow.into_owned())
}

/// 这个文件看起来像存档吗？
///
/// 两级判定，避免通用扩展名误伤：
/// - **存档专属扩展名**（`sav`/`rpgsave`/`rvdata2`…）→ 直接算；
/// - **通用扩展名**（`dat`/`bin`/`json`…）→ 还要文件名含存档语义（`save`/`system`/`data`…）。
fn looks_like_save_ext(p: &Path) -> bool {
    let Some(ext) = p.extension().and_then(|x| x.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    if SAVE_EXTS.contains(&ext.as_str()) {
        return true;
    }
    if GENERIC_EXTS.contains(&ext.as_str()) {
        return looks_like_save_name(p);
    }
    false
}

/// 文件名（不含扩展名）里是否带存档语义。配合 [`GENERIC_EXTS`] 使用。
fn looks_like_save_name(p: &Path) -> bool {
    let name = p
        .file_stem()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name_matches(&name, SAVE_NAME_HINTS)
}

/// 候选**目标**存档目录：策略声明优先，再补一份"名字像存档目录"的根级猜测。
///
/// 两条硬约束：
/// - **游戏根不算存档目录**（`"."` 提示被忽略）：把存档丢进游戏根往往放错位置，
///   需要显式 `--opt:save_dir=<游戏根>` 才会用它；
/// - **排除「自带全CG存档」目录**：那是*来源*不是*目标*，否则会出现"复制到自身"。
///   判据与 `find_bundled_saves` 共用 [`is_bundle_dir_name`] —— 两边各写一套
///   就会出现「一个认来源、一个认目标」的自相矛盾（`C0771存档` 那次正是如此）。
///
/// 另外，如果游戏目录里能找到 `位置.txt` 这类说明，**优先采信它写的路径**
/// （那是打包者亲手写的目标目录，比任何名字启发式都准）。
///
/// 只返回**确实存在**的目录。
pub fn find_save_dirs(root: &Path, spec: &UnlockSpec) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    // 用规范化路径去重：Windows 文件系统大小写不敏感，
    // `SaveData` 与 `savedata` 会解析到同一目录，字符串比较去不掉这个重复。
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let root_key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut push = |p: PathBuf| {
        if !p.is_dir() {
            return;
        }
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        if key == root_key {
            return; // 游戏根不算"存档目录"
        }
        if seen.insert(key) {
            out.push(p);
        }
    };

    // ---- ① 说明文件里写明的目标（最准，放最前）----
    if let Some(d) = find_declared_save_dir(root) {
        push(d.0);
    }

    // ---- ② 策略表声明的候选 + 名字像存档的目录 ----
    //
    // 递归找（不再只看根一层）：分享包与游戏本体都可能把存档目录埋在子目录里，
    // 只扫一层会漏。深度给上限，避免在超大素材目录里空转。
    let mut guess_dirs: Vec<PathBuf> = Vec::new();
    let mut seen_dirs = 0usize;
    for e in WalkDir::new(root).max_depth(4).follow_links(false).into_iter().flatten() {
        seen_dirs += 1;
        if seen_dirs > 20_000 {
            break;
        }
        if !e.file_type().is_dir() || e.depth() == 0 {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        // 来源目录（自带存档包）不是目标，跳过一整棵子树。
        if is_bundle_dir_name(&name) {
            continue;
        }
        if name_matches(&name, SAVE_DIR_KEYWORDS) {
            guess_dirs.push(e.path().to_path_buf());
        }
    }
    // 策略表声明的相对目录：也要过一遍来源判据（策略表里可能有 `.` 之类）。
    for hint in spec.save_dirs {
        if *hint == "." || hint.is_empty() {
            continue;
        }
        let p = root.join(hint);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if is_bundle_dir_name(&name) {
            continue;
        }
        push(p);
    }
    for p in guess_dirs {
        push(p);
    }
    out
}

/// 在游戏目录里找「自带全CG存档」（网盘分享包常见）。
///
/// 两类命中：
/// 1. **目录**名判为自带存档来源（[`is_bundle_dir_name`]：含 `全CG/全开/100%/回想/回廊`
///    或**泛化的 `<任意名>存档`**，如 `C0771存档`）→ **递归**收该目录下的存档文件；
/// 2. **文件**名含 `全CG/全开/clear/gallery/complete` 等 **且** 扩展名像存档。
///
/// ## 为什么是「递归到底」而不是只看一层
///
/// 分享包的嵌套层数**没有上限**，实测就遇到过
/// `C0771存档/C0771存档/存档/System.bin`（三层，每层都叫存档）。
/// 老实现 `max_depth(3)` + 逐层名字匹配，**任何一层名字不匹配就整条链断掉** →
/// 明明有存档却报「0 个」。现在改成：**目录名一旦判定为来源，就递归收底下所有
/// 「像存档」的文件**，不再要求每层都命中关键字。
///
/// 深度仍给上限（8 层）与文件数上限（2 万），避免在超大素材目录里空转。
pub fn find_bundled_saves(root: &Path) -> Vec<PathBuf> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    let mut seen = 0usize;
    // 收集「判定为来源」的目录，稍后递归收文件。
    let mut src_dirs: Vec<PathBuf> = Vec::new();

    for e in WalkDir::new(root).max_depth(4).follow_links(false).into_iter().flatten() {
        seen += 1;
        if seen > 20_000 {
            break;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        if e.file_type().is_dir() {
            if e.depth() == 0 {
                continue; // 游戏根自身不算「自带存档目录」
            }
            if is_bundle_dir_name(&name) {
                src_dirs.push(e.path().to_path_buf());
            }
        } else if e.file_type().is_file()
            && name_matches(&name, BUNDLE_FILE_KEYWORDS)
            && looks_like_save_ext(e.path())
        {
            out.insert(e.path().to_path_buf());
        }
    }

    // 递归收每个来源目录下的存档文件。
    for d in src_dirs {
        let mut preferred: Vec<PathBuf> = Vec::new();
        let mut rest: Vec<PathBuf> = Vec::new();
        for e in WalkDir::new(&d).max_depth(8).follow_links(false).into_iter().flatten() {
            if !e.file_type().is_file() {
                continue;
            }
            let p = e.path();
            // 读不到 metadata = 文件可能已消失或权限不足，**不能当超大文件跳过**
            // （那会让真正存在的存档凭空不见）；这里只跳过「确认超大」的。
            let too_big = match p.metadata() {
                Ok(m) => m.len() > 64 * 1024 * 1024,
                Err(_) => false,
            };
            if too_big {
                continue; // 超大文件绝不是存档（多半是视频/音频）
            }
            if looks_like_save_ext(p) {
                preferred.push(p.to_path_buf());
            } else if is_location_file(p) {
                rest.push(p.to_path_buf()); // 说明文件垫底（用户可能想看一眼）
            }
        }
        // 优先只收「像存档」的；一个都没有时才退回收说明文件 ——
        // 免得把 `位置.txt` 当成要复制的存档。
        for p in if preferred.is_empty() { rest } else { preferred } {
            out.insert(p);
        }
    }
    out.into_iter().collect()
}

/// 是不是「存档说明文件」（`位置.txt` 之类）。
fn is_location_file(p: &Path) -> bool {
    p.file_name()
        .map(|n| {
            let n = n.to_string_lossy().to_lowercase();
            LOCATION_FILE_NAMES.iter().any(|k| n == k.to_lowercase())
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// 路线选择
// ---------------------------------------------------------------------------

/// 选择实际执行的路线。
///
/// 规则（顺序敏感，与 skill 的「通用解锁路线」一致）：
/// 1. `--opt:route=` 显式指定 → 用它；
/// 2. 带 `--opt:restore=` → 用该引擎的首选（引擎私有）路线；
/// 3. 发现「自带全CG存档」→ `BundledSave`（与引擎无关、最省事）；
/// 4. 否则用策略表的第一条。
pub fn pick_route(
    spec: &UnlockSpec,
    has_bundled: bool,
    explicit: Option<UnlockRoute>,
    restore: bool,
) -> UnlockRoute {
    if let Some(r) = explicit {
        return r;
    }
    if restore {
        return spec.routes.first().copied().unwrap_or(UnlockRoute::Manual);
    }
    if has_bundled {
        return UnlockRoute::BundledSave;
    }
    spec.routes.first().copied().unwrap_or(UnlockRoute::Manual)
}

// ---------------------------------------------------------------------------
// 统一调度
// ---------------------------------------------------------------------------

/// `Op::Unlock` 的统一入口。`engine_impl` 由各引擎提供其私有实现
/// （返回 `None` 表示该手段无私有实现，由本模块兜底）。
///
/// 选项（`--opt:key=value`）：
/// - `apply=1`：真正落地；缺省为**只读预览**（安全默认）
/// - `route=<key>`：指定手段（bundled / ingame / config / registry / savefile / manual）
/// - `save_dir=<路径>`：覆盖"替换自带存档"的目标目录
/// - 其余选项原样透传给引擎私有实现（如 Unity 的 `filter` / `restore`、Ren'Py 的 `set_true_all`）
pub fn run<F>(engine_id: &str, ctx: &Ctx, mut engine_impl: F) -> OpOutcome
where
    F: FnMut(UnlockRoute) -> Option<OpOutcome>,
{
    let spec = spec_or_generic(engine_id);
    let apply = ctx.opt("apply") == Some("1");

    let explicit = match ctx.opt("route") {
        Some(k) => match UnlockRoute::from_key(k) {
            Some(r) => Some(r),
            None => {
                return OpOutcome::fail(format!(
                    "未知 route={k}；可选: {}",
                    route_keys()
                ))
            }
        },
        None => None,
    };

    let bundled = find_bundled_saves(ctx.root);
    let restore = ctx.opt("restore").is_some();
    let route = pick_route(spec, !bundled.is_empty(), explicit, restore);

    // ---- 路线 1：替换自带全CG存档（与引擎无关）----
    if route == UnlockRoute::BundledSave {
        if bundled.is_empty() {
            let mut m = preview_msg(spec, engine_id, ctx, &bundled);
            m.push_str(
                "\n\n✘ 未发现《自带全CG存档》。请改用 --opt:route=<手段>，\
                 或用 --opt:save_dir=<目录> 指定目标后手动复制存档。",
            );
            // 只读预览算"正常返回"，apply 模式下算失败（没做成事）
            return OpOutcome {
                success: !apply,
                message: m,
                files_done: 0,
                logs: vec![],
            };
        }
        return bundled_route(ctx, spec, &bundled, apply);
    }

    // ---- 只读模式：优先交给引擎私有实现（其预览更精确）----
    if !apply {
        if let Some(o) = engine_impl(route) {
            return o;
        }
        return OpOutcome::ok(preview_msg(spec, engine_id, ctx, &bundled));
    }

    // ---- 落地模式 ----
    if let Some(o) = engine_impl(route) {
        return o;
    }
    // 无私有实现：给出明确指引（诚实告知"需人工"）
    OpOutcome::fail(format!(
        "「{}」的『{}』手段没有内置自动实现。\n{}\n\n{}\n\
         可尝试：--opt:route=bundled（替换自带存档）/ manual（人工分析）。",
        engine_id,
        route.label(),
        spec.action,
        route.desc()
    ))
}

/// 统一预览报告。
fn preview_msg(spec: &UnlockSpec, engine_id: &str, ctx: &Ctx, bundled: &[PathBuf]) -> String {
    let dirs = find_save_dirs(ctx.root, spec);
    let mut m = format!(
        "【解锁预览 · 只读】（引擎 {engine_id}）\n\
         识别依据: {}\n\
         解锁动作: {}\n\
         可用手段（--opt:route=<key>）:",
        spec.basis, spec.action
    );
    for r in spec.routes {
        m.push_str(&format!(
            "\n  · {:<9} {:<16} [{}] {}",
            r.key(),
            r.label(),
            if r.automated() { "自动" } else { "仅指引" },
            r.desc()
        ));
    }
    m.push_str(&format!("\n自带全CG存档候选: {} 个", bundled.len()));
    if !bundled.is_empty() {
        let sample: Vec<String> = bundled
            .iter()
            .take(8)
            .map(|p| {
                p.strip_prefix(ctx.root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        m.push_str(&format!("\n  示例: {sample:?}"));
    }
    if dirs.is_empty() {
        m.push_str("\n存档目录: 未找到（先运行一次游戏生成存档目录，或用 --opt:save_dir= 指定）");
    } else {
        let shown: Vec<String> = dirs
            .iter()
            .take(8)
            .map(|p| {
                p.strip_prefix(ctx.root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        m.push_str(&format!("\n存档目录: {} 个 —— {shown:?}", dirs.len()));
    }
    if !spec.note.is_empty() {
        m.push_str(&format!("\n提示: {}", spec.note));
    }
    m.push_str("\n→ 确认后用 --opt:apply=1 执行；覆盖写盘前会自动备份（.stool.bak），可用 stool restore 还原。");
    m
}

/// 体检自带存档包的内容，回答一个关键问题：**复制过去到底能不能解锁**。
///
/// 起因：`くりくりクリニック` 的自带包 `C0771存档` 里**只有 `System.bin`**（音量 / 跳过 /
/// 已读记录这类设置），CG 标记 `mNNN_NNFlag` 却在真正的进度存档 `data*.bin` 里。
/// 用户按提示复制完发现「没解锁」，会以为是工具坏了 —— 所以这里**预览时就说清**。
///
/// 判据刻意保守（宁可不说，不可说错）：
/// - 候选文件里出现 `Flag` 字样、或文件名含 `data`/`save`（进度存档的典型命名）→ 认为有进度数据，不提示；
/// - 全部候选都是纯设置文件 → 给出「多半解不开 CG」的提示，并说明标记在哪。
fn bundled_content_warning(candidates: &[PathBuf]) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    // 进度存档的典型特征：文件名带 data/globals/save 这类词，或内含 `Flag` 字面量。
    const PROGRESS_NAME_HINTS: &[&str] = &["data", "globals", "save", "progress", "game"];
    let mut looks_progress = false;
    let mut scanned = 0usize;
    for c in candidates.iter().take(32) {
        scanned += 1;
        let name = c
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if PROGRESS_NAME_HINTS.iter().any(|h| name.contains(h)) {
            looks_progress = true;
            break;
        }
        // 读前 512 KiB 找 `Flag` 字面量（进度存档里普遍是 `<场景>_<序号>Flag` 这种键名）。
        if let Ok(b) = fs::read(c) {
            let head = &b[..b.len().min(512 * 1024)];
            if head.windows(4).any(|w| w == b"Flag") {
                looks_progress = true;
                break;
            }
        }
    }
    if looks_progress || scanned == 0 {
        return None;
    }
    Some(
        concat!(
            "\n\n⚠ 这个自带包里没看到「进度存档」的迹象（只有设置类文件）。\n",
            "· CG / 回想标记通常存在进度存档里",
            "（例如 Unity 常见 `data*.bin` 里的 `…Flag` 字段），\n",
            " 而设置文件（音量 / 跳过 / 已读记录）里没有这些标记。\n",
            "· 若复制后仍未解锁，请改用 `--opt:route=registry` 走注册表，",
            "或用游戏内全开开关。"
        )
        .to_string(),
    )
}

/// 「替换自带全CG存档」路线：预览或落地。
fn bundled_route(ctx: &Ctx, spec: &UnlockSpec, candidates: &[PathBuf], apply: bool) -> OpOutcome {
    let targets = save_target_dirs(ctx, spec);

    if !apply {
        let mut m = format!(
            "【自带全CG存档 · 只读预览】发现 {} 个候选文件（未复制）",
            candidates.len()
        );
        for (i, p) in candidates.iter().take(20).enumerate() {
            m.push_str(&format!(
                "\n  {}. {}",
                i + 1,
                p.strip_prefix(ctx.root).unwrap_or(p).to_string_lossy()
            ));
        }
        if candidates.len() > 20 {
            m.push_str(&format!("\n  …（共 {} 个）", candidates.len()));
        }
        if targets.is_empty() {
            m.push_str(
                "\n目标存档目录: 未找到 —— 请先运行一次游戏生成存档目录，或加 --opt:save_dir=<路径> 指定。",
            );
        } else {
            let shown: Vec<String> = targets
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            m.push_str(&format!("\n目标存档目录: {shown:?}"));
        }
        m.push_str("\n→ 确认后加 --opt:apply=1 复制；被覆盖的存档会先备份为 .stool.bak。");
        // 内容体检：自带包里可能只有「设置文件」而没有「进度存档」，此时复制过去解不开 CG。
        // 网盘分享包常见这种缺斤少两的情况，必须当场说清，不能让人以为跑完就好了。
        if let Some(warn) = bundled_content_warning(candidates) {
            m.push_str(&warn);
        }
        return OpOutcome::okn(m, candidates.len());
    }

    if targets.is_empty() {
        return OpOutcome::fail(
            "未找到存档目录：请先运行一次游戏生成存档目录，或用 --opt:save_dir=<路径> 指定后再试。",
        );
    }

    let mut done = 0usize;
    let mut failed = 0usize;
    let mut backed = 0usize;
    let mut logs: Vec<String> = Vec::new();
    for c in candidates {
        if ctx.cancelled() {
            return OpOutcome::fail("已取消（部分文件可能已复制）");
        }
        let fname = match c.file_name() {
            Some(n) => n,
            None => continue,
        };
        // 目标目录：候选所在目录名与某目标目录同名则对齐，否则用第一个
        let src_dir_name = c
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let target = targets
            .iter()
            .find(|d| {
                d.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase() == src_dir_name)
                    .unwrap_or(false)
            })
            .unwrap_or(&targets[0]);
        let dst = target.join(fname);
        if dst == *c {
            continue; // 候选本来就在目标目录里，无需复制
        }
        if dst.exists() {
            match crate::settings::backup_once(&dst) {
                Ok(b) => {
                    backed += 1;
                    logs.push(format!("已备份 {} → {}", dst.display(), b.display()));
                }
                Err(e) => {
                    failed += 1;
                    crate::diag::log("WARN", &format!("unlock: 备份失败 {e}，跳过 {}", dst.display()));
                    continue;
                }
            }
        }
        match fs::copy(c, &dst) {
            Ok(_) => {
                done += 1;
                ctx.report(done as f32 / candidates.len().max(1) as f32, &dst.display().to_string());
            }
            Err(e) => {
                failed += 1;
                crate::diag::log("WARN", &format!("unlock: 复制失败 {}: {e}", dst.display()));
            }
        }
    }

    let mut m = format!(
        "✔ 已替换自带全CG存档：复制 {done} 个文件（失败 {failed}，覆盖前备份 {backed} 个）\n\
         目标目录: {:?}",
        targets.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
    );
    if failed > 0 {
        m.push_str(&format!("\n⚠ 有 {failed} 个文件未复制成功，详见日志"));
    }
    m.push_str("\n提示：启动游戏查看画廊/CG 回廊是否全开；若仍缺项，说明该存档不含全部条目。");
    OpOutcome {
        success: done > 0 && failed == 0,
        message: m,
        files_done: done,
        logs,
    }
}

/// 目标存档目录：`--opt:save_dir=` 覆盖优先，否则按策略表 + 目录名猜测。
fn save_target_dirs(ctx: &Ctx, spec: &UnlockSpec) -> Vec<PathBuf> {
    if let Some(d) = ctx.opt("save_dir") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return vec![p];
        }
    }
    find_save_dirs(ctx.root, spec)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_unlock_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 用假 Ctx 跑一遍（progress/cancel 都是 no-op）。
    fn with_ctx<F: FnOnce(&Ctx) -> OpOutcome>(root: &Path, opts: &[(&str, &str)], f: F) -> OpOutcome {
        let out = root.join("_out");
        let _ = fs::create_dir_all(&out);
        let map: std::collections::HashMap<String, String> = opts
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let cancel = AtomicBool::new(false);
        let ctx = Ctx {
            root,
            out_dir: &out,
            options: &map,
            progress: &|_, _| {},
            cancel: &cancel,
        };
        f(&ctx)
    }

    #[test]
    fn catalog_ids_unique_and_cover_known_engines() {
        let mut ids: Vec<&str> = UNLOCK_CATALOG.iter().map(|s| s.engine_id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "策略表存在重复 engine_id");
        for id in [
            "renpy", "rpgmaker_mv", "rpgmaker_rgss", "kirikiri", "godot", "nscripter",
            "tyrano", "html_game", "wolf", "unity", "bgi_ethornell", "artemis", "unreal",
            "cocos2dx", "flash", "qsp",
        ] {
            assert!(spec_for(id).is_some(), "策略表缺少引擎 {id}");
        }
    }

    #[test]
    fn every_spec_has_bundled_or_manual_fallback() {
        for s in UNLOCK_CATALOG {
            assert!(!s.routes.is_empty(), "{} 未声明任何手段", s.engine_id);
            assert!(
                s.routes.contains(&UnlockRoute::BundledSave)
                    || s.routes.contains(&UnlockRoute::Manual),
                "{} 既无自带存档也无人工兜底",
                s.engine_id
            );
        }
    }

    #[test]
    fn route_key_roundtrip() {
        for r in UnlockRoute::all() {
            assert_eq!(UnlockRoute::from_key(r.key()), Some(*r), "{:?} 的 key 不能反解", r);
        }
        // 别名
        assert_eq!(UnlockRoute::from_key("persistent"), Some(UnlockRoute::SaveFileFlag));
        assert_eq!(UnlockRoute::from_key("playerprefs"), Some(UnlockRoute::Registry));
        assert_eq!(UnlockRoute::from_key("nonsense"), None);
    }

    #[test]
    fn pick_route_priority() {
        let unity = spec_for("unity").unwrap();
        // 显式最高
        assert_eq!(
            pick_route(unity, true, Some(UnlockRoute::Manual), false),
            UnlockRoute::Manual
        );
        // 自带存档次之（即便 unity 首选是 registry）
        assert_eq!(pick_route(unity, true, None, false), UnlockRoute::BundledSave);
        // 无自带存档 → 用首选（unity = registry）
        assert_eq!(pick_route(unity, false, None, false), UnlockRoute::Registry);
        // restore → 引擎首选
        assert_eq!(pick_route(unity, false, None, true), UnlockRoute::Registry);
    }

    #[test]
    fn find_bundled_detects_cg_save_dir_and_named_file() {
        let d = tmpdir("bundled");
        // 目录式：全CG存档/SystemData.dat
        fs::create_dir_all(d.join("全CG存档")).unwrap();
        fs::write(d.join("全CG存档").join("SystemData.dat"), b"S").unwrap();
        fs::write(d.join("全CG存档").join("readme.txt"), b"x").unwrap(); // 非存档扩展名应被过滤
        // 文件式：clear_all.sav
        fs::write(d.join("clear_all.sav"), b"C").unwrap();
        let found = find_bundled_saves(&d);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"SystemData.dat".to_string()), "{names:?}");
        assert!(!names.contains(&"readme.txt".to_string()), "非存档扩展名不该入选");
        assert!(names.contains(&"clear_all.sav".to_string()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn find_save_dirs_uses_hints_and_keywords() {
        let d = tmpdir("savedirs");
        fs::create_dir_all(d.join("SaveData")).unwrap();
        fs::create_dir_all(d.join("Music")).unwrap(); // 非存档
        fs::create_dir_all(d.join("mydata")).unwrap();
        fs::create_dir_all(d.join("全CG存档")).unwrap(); // 来源，不是目标
        let spec = spec_for("unity").unwrap(); // hints: . / saves / SaveData
        let dirs = find_save_dirs(&d, spec);
        assert!(!dirs.contains(&d), "游戏根不应算存档目录");
        // Windows 大小写不敏感 FS 上 `SaveData` 与提示里的 `savedata` 会解析到同一目录，
        // 规范化去重后应只剩一个。
        assert_eq!(dirs.len(), 1, "应去重为 1 个：{dirs:?}");
        assert_eq!(
            dirs[0].file_name().unwrap().to_string_lossy().to_ascii_lowercase(),
            "savedata"
        );
        assert!(!dirs.contains(&d.join("Music")));
        assert!(!dirs.contains(&d.join("mydata")), "mydata 不含 save/存档 关键字");
        assert!(
            !dirs.contains(&d.join("全CG存档")),
            "自带存档目录是来源，不能当目标（否则会复制到自身）"
        );

        // 关键字猜测：unity 的提示里**没有** savedata，只能靠目录名关键字命中
        let d2 = tmpdir("savedirs_kw");
        fs::create_dir_all(d2.join("savedata")).unwrap();
        fs::create_dir_all(d2.join("Music")).unwrap();
        let dirs2 = find_save_dirs(&d2, spec);
        assert_eq!(dirs2.len(), 1, "关键字猜测应命中 savedata：{dirs2:?}");
        assert_eq!(
            dirs2[0].file_name().unwrap().to_string_lossy().to_ascii_lowercase(),
            "savedata"
        );
        let _ = fs::remove_dir_all(&d2);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn default_preview_is_read_only_and_ok() {
        let d = tmpdir("preview");
        fs::create_dir_all(d.join("Game_Data")).unwrap();
        fs::write(d.join("UnityPlayer.dll"), b"x").unwrap();
        let r = with_ctx(&d, &[], |ctx| run("unity", ctx, |_| None));
        assert!(r.success, "缺省应为只读预览且成功：{}", r.message);
        assert!(r.message.contains("只读"), "{}", r.message);
        // 没有写入任何文件
        assert!(!d.join("UnityPlayer.dll.stool.bak").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn bundled_apply_copies_and_backs_up() {
        let d = tmpdir("bundled_apply");
        fs::create_dir_all(d.join("全CG存档")).unwrap();
        fs::write(d.join("全CG存档").join("SystemData.dat"), b"NEW").unwrap();
        fs::create_dir_all(d.join("SaveData")).unwrap();
        fs::write(d.join("SaveData").join("SystemData.dat"), b"OLD").unwrap();

        let r = with_ctx(&d, &[("apply", "1"), ("route", "bundled")], |ctx| {
            run("bgi_ethornell", ctx, |_| None)
        });
        assert!(r.success, "{}", r.message);
        assert_eq!(r.files_done, 1);
        // 目标被覆盖，但原内容进了 .stool.bak
        assert_eq!(fs::read(d.join("SaveData").join("SystemData.dat")).unwrap(), b"NEW");
        assert_eq!(
            fs::read(d.join("SaveData").join("SystemData.dat.stool.bak")).unwrap(),
            b"OLD",
            "覆盖前必须备份原存档"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn bundled_apply_without_save_dir_fails_cleanly() {
        let d = tmpdir("bundled_nodir");
        fs::create_dir_all(d.join("全CG存档")).unwrap();
        fs::write(d.join("全CG存档").join("SystemData.dat"), b"NEW").unwrap();
        // 没有任何存档目录，也没有 --opt:save_dir
        let r = with_ctx(&d, &[("apply", "1"), ("route", "bundled")], |ctx| {
            run("bgi_ethornell", ctx, |_| None)
        });
        assert!(!r.success, "缺存档目录时不应报成功：{}", r.message);
        assert!(r.message.contains("save_dir"), "{}", r.message);
        let _ = fs::remove_dir_all(&d);
    }

    // ---- くりくりクリニック 实录：泛化「xx存档」+ 三层嵌套 + 位置.txt ----

    #[test]
    fn generic_archive_dir_name_is_recognized() {
        // 命中：`<任意名>存档`（分享包常带社团编号前缀）
        assert!(is_bundle_dir_name("c0771存档"));
        assert!(is_bundle_dir_name("存档"));
        assert!(is_bundle_dir_name("某作存档"));
        // 老关键字仍然命中
        assert!(is_bundle_dir_name("全cg存档"));
        assert!(is_bundle_dir_name("全开"));
        // 否决：备份 / 素材 / 引擎自己的保存目录
        assert!(!is_bundle_dir_name("存档备份"));
        assert!(!is_bundle_dir_name("素材存档"));
        assert!(!is_bundle_dir_name("savedata"));
        assert!(!is_bundle_dir_name("resources"));
        // 「存档」在中间不算（只认后缀）
        assert!(!is_bundle_dir_name("存档工具"));
    }

    #[test]
    fn nested_generic_archive_is_found_and_not_a_target() {
        // 实录目录：C0771存档/C0771存档/存档/System.bin（三层，每层都叫「存档」）。
        // 老实现要求每一层都命中关键字，且只看一层 → 报「0 个」并把来源当目标。
        let d = tmpdir("kkc_nested");
        let inner = d.join("C0771存档").join("C0771存档").join("存档");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("System.bin"), b"{}").unwrap();
        fs::write(inner.join("位置.txt"), "C:\\Users\\（用户名）\\AppData\\LocalLow\\X\\KKC4").unwrap();

        let found = find_bundled_saves(&d);
        assert_eq!(found.len(), 1, "三层嵌套的自带存档没被找到：{found:?}");
        assert!(found[0].ends_with("System.bin"), "{:?}", found[0]);

        // 来源目录绝不能同时当目标（否则会「复制到自身」）
        let spec = spec_for("unity").unwrap();
        let dirs = find_save_dirs(&d, spec);
        assert!(
            !dirs.iter().any(|p| p.to_string_lossy().contains("C0771")),
            "自带存档目录被误认成目标存档目录：{dirs:?}"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn generic_ext_needs_save_semantics() {
        // `json`/`bin`/`dat` 是引擎自己也在用的扩展名。只有文件名带存档语义才算候选，
        // 否则 `catalog.json` / `uninst000.dat` 这种会被误收。
        let d = tmpdir("kkc_generic_ext");
        fs::create_dir_all(&d).unwrap();
        let ok_sys = d.join("System.bin");
        let ok_data = d.join("data35.bin");
        let bad_catalog = d.join("catalog.json");
        let bad_unins = d.join("unins000.dat");
        let ok_sav = d.join("anything.sav");
        for p in [&ok_sys, &ok_data, &bad_catalog, &bad_unins, &ok_sav] {
            fs::write(p, b"{}").unwrap();
        }

        assert!(looks_like_save_ext(&ok_sys), "System.bin 应算存档");
        assert!(looks_like_save_ext(&ok_data), "data35.bin 应算存档");
        assert!(looks_like_save_ext(&ok_sav), ".sav 是存档专属扩展名");
        assert!(
            !looks_like_save_ext(&bad_catalog),
            "catalog.json 没有存档语义，不该算存档"
        );
        assert!(
            !looks_like_save_ext(&bad_unins),
            "unins000.dat 没有存档语义，不该算存档"
        );

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn save_dir_is_found_when_nested() {
        // 存档目录可能埋在子目录里（不只根一层）。旧实现只 `read_dir(root)` 一层，
        // 嵌套的就找不到 → 「未找到存档目录」。
        let d = tmpdir("kkc_nested_target");
        let nested = d.join("game").join("data");
        fs::create_dir_all(&nested).unwrap();
        // 名字命中 SAVE_DIR_KEYWORDS，且不是「来源目录」
        let target = nested.join("SaveData");
        fs::create_dir_all(&target).unwrap();

        let spec = spec_for("unity").unwrap();
        let dirs = find_save_dirs(&d, spec);
        assert!(
            dirs.iter().any(|p| p.ends_with("SaveData")),
            "嵌套的存档目录没被找到：{dirs:?}"
        );

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn declared_location_file_wins_as_target() {
        // 说明文件里写的路径 > 任何名字启发式。这里用一个「已存在的」真实目录
        // 作为目标（位置文件指向它），验证它排在候选最前。
        let d = tmpdir("kkc_locate");
        let src = d.join("C0771存档").join("存档");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("System.bin"), b"{}").unwrap();
        let target = d.join("SaveData");
        fs::create_dir_all(&target).unwrap();

        let txt = "C:\\Users\\（用户名）\\AppData\\LocalLow\\X\\KKC4\n还是中文说明行";
        fs::write(src.join("位置.txt"), txt.as_bytes()).unwrap();

        // 直接验证解析器（目标目录可能在本机不存在，find_save_dirs 只收存在的）
        let (p, raw) = parse_save_location(
            "C:\\Users\\（用户名）\\AppData\\LocalLow\\X\\KKC4\n说明：解压后覆盖",
        )
        .expect("应能解析出绝对路径行");
        let user = std::env::var("USERNAME").unwrap_or_default();
        assert!(!p.to_string_lossy().contains("用户名"), "占位符没被替换：{p:?}");
        if !user.is_empty() {
            assert!(p.to_string_lossy().contains(&user), "没替换成当前用户名：{p:?}");
        }
        assert!(raw.contains("LocalLow"));
        // 中文说明行不该被当成路径
        assert!(parse_save_location("说明：解压后覆盖到存档目录").is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn gbk_location_file_decodes_and_expands_username() {
        // 实录字节：`位置.txt` 是 GBK，`（用户名）` = A3 A8 D3 C3 BB A7 C3 FB A3 A9。
        // 早先按 `decode()` 的 ok=false 退回 from_utf8_lossy → 汉字全变 U+FFFD
        // → 占位符匹配不上 → 目标目录凭空消失。
        let bytes: Vec<u8> = {
            let mut v = b"C:\\Users\\".to_vec();
            v.extend_from_slice(&[0xA3, 0xA8, 0xD3, 0xC3, 0xBB, 0xA7, 0xC3, 0xFB, 0xA3, 0xA9]); // （用户名）
            v.extend_from_slice(b"\\AppData\\LocalLow\\GIRL'S SOFTWARE\\KKC4");
            v
        };
        let text = decode_text(&bytes).expect("GBK 说明文件应能解码");
        assert!(text.contains("（用户名）"), "GBK 汉字没解出来：{text:?}");
        assert!(!text.contains('\u{fffd}'), "出现了替换字符：{text:?}");

        let (p, _) = parse_save_location(&text).expect("应解析出绝对路径");
        let user = std::env::var("USERNAME").unwrap_or_default();
        if !user.is_empty() {
            assert!(p.to_string_lossy().contains(&user), "用户名没替换：{p:?}");
        }
        assert!(p.to_string_lossy().ends_with("KKC4"));
    }

    #[test]
    fn settings_only_bundle_gets_warning() {
        // 实录：`C0771存档` 只有 `System.bin`（16 个键全是音量/跳过/已读记录），
        // 没有 `Flag`、名字里也没有 data/save → 必须提示「多半解不开 CG」。
        let d = tmpdir("kkc_settings_only");
        fs::create_dir_all(&d).unwrap();
        let sys = d.join("System.bin");
        fs::write(
            &sys,
            br#"{"messageSpeed":0.5,"bgmVolume":0.5,"passingScene":["M000_01_h"]}"#,
        )
        .unwrap();

        let w = bundled_content_warning(&[sys]).expect("纯设置文件应当给出提示");
        assert!(w.contains("进度存档"), "{w}");
        assert!(w.contains("registry"), "提示里要给出替代出路：{w}");

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn progress_save_bundle_gets_no_warning() {
        // 带 `Flag` 字面量（或文件名含 data）的候选 = 真进度存档 → 不该干扰用户。
        let d = tmpdir("kkc_progress");
        fs::create_dir_all(&d).unwrap();
        let with_flag = d.join("globals.bin");
        fs::write(&with_flag, br#"{"m001_04Flag":true}"#).unwrap();
        assert!(bundled_content_warning(&[with_flag]).is_none());

        let by_name = d.join("data35.bin");
        fs::write(&by_name, b"\x00\x01\x02").unwrap();
        assert!(
            bundled_content_warning(&[by_name]).is_none(),
            "文件名含 data 就应视为进度存档"
        );

        assert!(bundled_content_warning(&[]).is_none(), "空列表不该报警");

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn location_file_found_at_depth_four() {
        // 实录：`位置.txt` 在 depth=4。早先 `max_depth(3)` 让它永远看不到 ——
        // 解析函数单测全绿，真机上却「目标目录: 未找到」。
        let d = tmpdir("kkc_depth4");
        let inner = d.join("A存档").join("A存档").join("存档");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("System.bin"), b"{}").unwrap();
        fs::write(inner.join("位置.txt"), "C:\\Users\\（用户名）\\AppData\\LocalLow\\X\\KKC4").unwrap();

        let hit = find_declared_save_dir(&d);
        assert!(hit.is_some(), "depth=4 的说明文件没被找到（又被深度限制挡住了？）");
        let (_, raw) = hit.unwrap();
        assert!(raw.contains("LocalLow"), "{raw}");

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_route_is_rejected() {
        let d = tmpdir("badroute");
        let r = with_ctx(&d, &[("route", "whatever")], |ctx| run("unity", ctx, |_| None));
        assert!(!r.success);
        assert!(r.message.contains("未知 route"), "{}", r.message);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn engine_private_impl_is_used_for_non_bundled_route() {
        let d = tmpdir("private");
        // 无自带存档 → unity 默认走 registry → 交给 engine_impl
        let r = with_ctx(&d, &[], |ctx| {
            run("unity", ctx, |route| {
                Some(OpOutcome::ok(format!("engine-impl:{:?}", route)))
            })
        });
        assert!(r.success);
        assert!(r.message.contains("Registry"), "{}", r.message);
        let _ = fs::remove_dir_all(&d);
    }
}
