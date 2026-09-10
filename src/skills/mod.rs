//! 技能系统：文件为真相源 + SQLite 索引 + roster 按需注入 + 质量门。
//!
//! 目录布局：
//! - `<skills_dir>/<id>/SKILL.md` —— 技能全文（frontmatter + 正文），文件是唯一真相源
//! - `<db_path>` —— SQLite 索引（启用状态/来源/内置标记/内容哈希）
//!
//! 与旧 `prompts.rs` 的差异：技能不再全量拼接进 system 前缀，改为只注入启用技能的
//! roster（名称 + 描述），正文按需读取；自定义技能落盘落库，重启不丢失。

pub mod frontmatter;
pub mod gate;
pub mod inject;
pub mod store;

pub use store::{SkillInfo, SkillInput, SkillsManager};
