//! 字体注册表：扫描内置字库、按需解析、缓存。
//!
//! 重字体（8.4 MB 的 `STRK-Kaiti.json`）必须**懒加载**：启动只登记路径，
//! 真正排版到某款字体时才解析（Python 版正是靠这一点避免启动即吃掉上百 MB）。
//! 当前阶段只实现 `.jhf` 的立即加载，中文单线 JSON 的懒加载在 M2 补上。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::font::jhf::parse_hershey_jhf;
use crate::font::model::FontFamily;

/// 内置字库根目录：优先环境变量 `BIZHEN_FONTS_DIR`，否则用仓库内 `assets/fonts`。
///
/// 发布时可以把 `assets/fonts` 拷到可执行文件旁的 `fonts/`，届时这里再加
/// 「exe 同级目录」的查找分支。
pub fn fonts_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BIZHEN_FONTS_DIR") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fonts")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts"))
}

#[derive(Debug, Clone)]
pub struct FontEntry {
    pub name: String,
    pub kind: &'static str,
    pub path: PathBuf,
}

/// 字体注册表：`name -> 条目`，解析结果常驻内存。
#[derive(Default)]
pub struct FontRegistry {
    entries: BTreeMap<String, FontEntry>,
    loaded: BTreeMap<String, FontFamily>,
}

impl FontRegistry {
    /// 扫描内置字库（`hershey/*.jhf`）；解析失败的字体不登记。
    pub fn builtin() -> Self {
        let mut reg = Self::default();
        let root = fonts_dir();
        let hershey = root.join("hershey");
        if let Ok(read) = std::fs::read_dir(&hershey) {
            let mut files: Vec<PathBuf> = read
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "jhf").unwrap_or(false))
                .collect();
            files.sort();
            for path in files {
                let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                    continue;
                };
                reg.entries.insert(
                    stem.clone(),
                    FontEntry {
                        name: stem,
                        kind: "hershey",
                        path,
                    },
                );
            }
        }
        // 中文单线字库只登记路径，解析留到 M2（懒加载）。
        let stroke_json = root.join("stroke-json");
        if let Ok(read) = std::fs::read_dir(&stroke_json) {
            for e in read.filter_map(|e| e.ok()) {
                let path = e.path();
                if path.extension().map(|x| x == "json").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                        reg.entries.insert(
                            stem.clone(),
                            FontEntry {
                                name: stem,
                                kind: "stroke-json",
                                path,
                            },
                        );
                    }
                }
            }
        }
        reg
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn entries(&self) -> Vec<&FontEntry> {
        self.entries.values().collect()
    }

    pub fn get_entry(&self, name: &str) -> Option<&FontEntry> {
        self.entries.get(name)
    }

    /// 取（并缓存解析结果）一款字体；首次访问才解析（中文字库 8.4 MB，务必懒加载）。
    pub fn get(&mut self, name: &str) -> Option<&FontFamily> {
        if !self.loaded.contains_key(name) {
            let entry = self.entries.get(name)?.clone();
            let fam = match entry.kind {
                "hershey" => parse_hershey_jhf(&entry.path, Some(&entry.name)).ok(),
                "stroke-json" => {
                    crate::font::stroke_json::parse_stroke_json(&entry.path, Some(&entry.name)).ok()
                }
                _ => None,
            }?;
            self.loaded.insert(name.to_string(), fam);
        }
        self.loaded.get(name)
    }
}
