//! 字体注册表：扫描内置字库与用户字库、按需解析、缓存。
//!
//! 重字体（8.4 MB 的 `STRK-Kaiti.json`、`STRK-*.gfont`）必须**懒加载**：
//! 启动只登记路径，真正排版到某款字体时才解析。
//!
//! 扫描范围：
//! * 内置：`assets/fonts/hershey/*.jhf`、`assets/fonts/stroke-json/*.json`
//! * 用户：`~/.bizhen/fonts/*.{jhf,json,gfont}`（可用 `BIZHEN_FONT_DIR` 覆盖），
//!   界面上「导入字体」会把文件拷到这里。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::font::gfont;
use crate::font::jhf::parse_hershey_jhf;
use crate::font::model::FontFamily;

/// 内置字库根目录：优先环境变量 `BIZHEN_FONTS_DIR`，否则用仓库内 `assets/fonts`。
pub fn fonts_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BIZHEN_FONTS_DIR") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fonts")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts"))
}

/// 用户字库目录（导入的 `.gfont` / `.jhf` / `.json` 放这里）。
pub fn user_font_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BIZHEN_FONT_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(home).join(".bizhen").join("fonts")
}

#[derive(Debug, Clone)]
pub struct FontEntry {
    /// 内部键（文件名主干，唯一）
    pub name: String,
    /// 展示名（`.gfont` 取加密头里的真实名称）
    pub display: String,
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
    /// 扫描内置 + 用户字库；解析失败的字体不登记。
    pub fn builtin() -> Self {
        let mut reg = Self::default();
        let root = fonts_dir();
        reg.scan_hershey(&root.join("hershey"));
        reg.scan_stroke_json(&root.join("stroke-json"));
        reg.scan_user(&user_font_dir());
        reg
    }

    fn scan_hershey(&mut self, dir: &Path) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<PathBuf> = read
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "jhf").unwrap_or(false))
            .collect();
        files.sort();
        for path in files {
            if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                self.register(&stem, "hershey", path, stem.clone());
            }
        }
    }

    fn scan_stroke_json(&mut self, dir: &Path) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for e in read.filter_map(|e| e.ok()) {
            let path = e.path();
            if path.extension().map(|x| x == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                    self.register(&stem, "stroke-json", path, stem.clone());
                }
            }
        }
    }

    /// 平铺扫描用户目录。
    fn scan_user(&mut self, dir: &Path) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<PathBuf> = read
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for path in files {
            let ext = path
                .extension()
                .map(|x| x.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            match ext.as_str() {
                "jhf" => self.register(&stem, "hershey", path, stem.clone()),
                "json" => self.register(&stem, "stroke-json", path, stem.clone()),
                _ => {}
            }
        }
    }

    fn register(&mut self, name: &str, kind: &'static str, path: PathBuf, display: String) {
        let mut key = name.to_string();
        let mut i = 2;
        while self.entries.contains_key(&key) {
            key = format!("{name}-{i}");
            i += 1;
        }
        self.entries.insert(
            key.clone(),
            FontEntry {
                name: key,
                display: if display.is_empty() {
                    name.to_string()
                } else {
                    display
                },
                kind,
                path,
            },
        );
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
                "gfont" => gfont::parse_gfont(&entry.path, Some(&entry.name)).ok(),
                _ => None,
            }?;
            self.loaded.insert(name.to_string(), fam);
        }
        self.loaded.get(name)
    }

    /// 重新扫描（导入新字体后调用），已解析缓存保留。
    pub fn refresh(&mut self) {
        let mut fresh = Self::builtin();
        fresh.loaded = std::mem::take(&mut self.loaded);
        *self = fresh;
    }
}