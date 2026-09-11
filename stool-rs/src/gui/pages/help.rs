//! 页面：help（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use eframe::egui::{RichText};

impl StoolApp {
    pub(crate) fn page_help(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "使用指南", "大部分操作都能靠“把文件/文件夹拖进窗口”完成");
        let sections: Vec<(&str, Vec<&str>)> = vec![
            ("🚀 快速上手", vec![
                "把游戏文件夹直接拖进本窗口 → 自动识别引擎；把存档文件拖进窗口 → 直接进入存档编辑器",
                "左侧页面按用途排列：解包资源、翻译文本、改存档、改运行中的游戏、装补丁",
            ]),
            ("📦 解包与汉化", vec![
                "资源解包页：点按钮即可把封包里的图片/音频/脚本提取到输出目录，不会动游戏原文件",
                "文本/汉化页分左右两条通道：CSV 通道=提取→机翻/人工→回填（改游戏资源文件）；JSON 通道=运行时注入（不改游戏文件）",
                "JSON 通道的原文来源：点\"从 CSV 生成 JSON 骨架\"自动把提取结果转成 {\"原文\":\"译文\"}（兼容 MTool 的 translation.json），也可手写",
                "机翻支持断点续翻与术语表（人名/术语译名全文一致）；人工翻译用 Excel/WPS 填 CSV 的 translation 列即可",
            ]),
            ("💾 存档编辑", vec![
                "打开存档文件（MV 的在游戏目录 save/ 下，后缀 .rpgsave）→ 自动识别格式",
                "搜索数值或键名（如金币数、角色名）→ 点击结果定位 → 改值 → 保存回写（自动备份 .stool.bak）",
                "支持导出/导入 JSON，方便备份或分享修改",
            ]),
            ("🎯 运行时修改", vec![
                "MV/MZ 游戏：首页检测后进入本页自动启动并连接，改金币/变量/物品立即生效",
                "其他游戏：用通用内存扫描，用法与 Cheat Engine 相同：首次扫描 → 变数值 → 再次扫描过滤 → 写入",
                "扫描完成后可以“锁定数值”（游戏里改不掉）或“撤销写入”（恢复原值）",
            ]),
            ("🧩 补丁 MOD", vec![
                "选择补丁文件夹一键覆盖安装，可随时停用/卸载，卸载时自动还原原文件",
            ]),
            ("⌨ 命令行", vec![
                "stool detect/extract/decompile/text-extract/text-import/save/unlock/mod-*",
                "stool save-edit 存档文件 --search 关键词 / --set /路径=新值（脚本批量改存档）",
            ]),
            ("❓ 常见问题", vec![
                "扫不到内存或打不开进程 → 以管理员身份运行 STool",
                "MV/MZ 已手动开着连不上 → 先关闭游戏再重试（避免多开）",
                "解包后文件在哪 → 底部状态栏点“📂 输出目录”直接打开",
            ]),
        ];
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (title, lines) in sections {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).strong());
                    for l in lines {
                        ui.label(RichText::new(l).small());
                    }
                });
                ui.add_space(2.0);
            }
        });
    }
}
