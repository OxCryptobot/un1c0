//! Bounded repository intelligence for agent context selection.
//!
//! The index is deliberately deterministic and local-first. It stores file
//! metadata and symbols, then reads only bounded snippets during retrieval.
//! It does not execute repository code or follow symlinks.

use crate::provider::ContextItem;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

const DEFAULT_MAX_FILES: usize = 20_000;
const DEFAULT_MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_SYMBOLS: usize = 100_000;
const DEFAULT_MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum RepositoryIndexError {
    #[error("repository index path error: {0}")]
    Path(String),
    #[error("repository index I/O error: {0}")]
    Io(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_symbols: usize,
    pub max_cached_bytes: usize,
    pub ignored_directories: BTreeSet<String>,
    pub include_extensions: BTreeSet<String>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_symbols: DEFAULT_MAX_SYMBOLS,
            max_cached_bytes: DEFAULT_MAX_CACHED_BYTES,
            ignored_directories: [
                ".git",
                ".hg",
                ".svn",
                "target",
                "node_modules",
                "vendor",
                "dist",
                "build",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            include_extensions: [
                "rs", "py", "js", "jsx", "ts", "tsx", "go", "zig", "swift", "move", "sol", "toml",
                "yaml", "yml", "json", "md", "txt",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    pub path: String,
    pub language: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedSymbol {
    pub path: String,
    pub language: String,
    pub kind: String,
    pub name: String,
    pub line: u32,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    pub language: Option<String>,
    pub max_results: usize,
    pub max_context_bytes: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            language: None,
            max_results: 20,
            max_context_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMatch {
    pub path: String,
    pub language: String,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    pub score: u32,
    pub symbol: Option<String>,
}

fn empty_content_cache() -> Arc<BTreeMap<String, String>> {
    Arc::new(BTreeMap::new())
}

fn empty_symbol_lookup() -> Arc<BTreeMap<String, BTreeMap<u32, String>>> {
    Arc::new(BTreeMap::new())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    pub root: PathBuf,
    pub files: Vec<IndexedFile>,
    pub symbols: Vec<IndexedSymbol>,
    pub total_bytes: u64,
    #[serde(skip, default = "empty_content_cache")]
    content_cache: Arc<BTreeMap<String, String>>,
    #[serde(skip, default = "empty_symbol_lookup")]
    symbol_lookup: Arc<BTreeMap<String, BTreeMap<u32, String>>>,
}

impl RepositoryIndex {
    pub fn build(
        root: impl AsRef<Path>,
        config: &IndexConfig,
    ) -> Result<Self, RepositoryIndexError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| RepositoryIndexError::Path(error.to_string()))?;
        if !root.is_dir() {
            return Err(RepositoryIndexError::Path(
                "index root is not a directory".into(),
            ));
        }
        let mut index = Self {
            root: root.clone(),
            files: Vec::new(),
            symbols: Vec::new(),
            total_bytes: 0,
            content_cache: empty_content_cache(),
            symbol_lookup: empty_symbol_lookup(),
        };
        let mut content_cache = BTreeMap::new();
        let mut cached_bytes = 0usize;
        walk_directory(
            &root,
            &root,
            config,
            &mut index,
            &mut content_cache,
            &mut cached_bytes,
        )?;
        index
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
        index.symbols.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.line.cmp(&right.line))
                .then(left.name.cmp(&right.name))
        });
        let mut symbol_lookup: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
        for symbol in &index.symbols {
            symbol_lookup
                .entry(symbol.path.clone())
                .or_default()
                .entry(symbol.line)
                .or_insert_with(|| symbol.name.clone());
        }
        index.content_cache = Arc::new(content_cache);
        index.symbol_lookup = Arc::new(symbol_lookup);
        Ok(index)
    }

    pub fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<ContextMatch>, RepositoryIndexError> {
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|token| token.to_lowercase())
            .filter(|token| !token.is_empty())
            .collect();
        if tokens.is_empty() || options.max_results == 0 || options.max_context_bytes == 0 {
            return Ok(Vec::new());
        }
        let mut candidates = Vec::new();
        for file in &self.files {
            if options
                .language
                .as_ref()
                .is_some_and(|language| !file.language.eq_ignore_ascii_case(language))
            {
                continue;
            }
            let path_score = score_text(&file.path, &tokens) * 2;
            let path = self.root.join(&file.path);
            let content = if let Some(content) = self.content_cache.get(&file.path) {
                Cow::Borrowed(content.as_str())
            } else {
                match fs::read_to_string(&path) {
                    Ok(content) => Cow::Owned(content),
                    Err(_) => continue,
                }
            };
            let lines: Vec<&str> = content.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                let line_score = score_text(line, &tokens);
                if line_score == 0 && path_score == 0 {
                    continue;
                }
                let symbol = self
                    .symbol_lookup
                    .get(&file.path)
                    .and_then(|symbols| symbols.get(&(index as u32 + 1)))
                    .cloned();
                let symbol_score = symbol
                    .as_ref()
                    .map(|name| score_text(name, &tokens) * 4)
                    .unwrap_or(0);
                let score = path_score + line_score * 5 + symbol_score;
                if score == 0 {
                    continue;
                }
                candidates.push(ContextMatch {
                    path: file.path.clone(),
                    language: file.language.clone(),
                    start_line: index as u32 + 1,
                    end_line: index as u32 + 1,
                    text: line.to_string(),
                    score,
                    symbol,
                });
            }
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then(left.path.cmp(&right.path))
                .then(left.start_line.cmp(&right.start_line))
        });
        let mut results = Vec::new();
        let mut bytes: usize = 0;
        for candidate in candidates {
            if results.len() >= options.max_results {
                break;
            }
            let candidate_bytes = candidate.text.len();
            if bytes.saturating_add(candidate_bytes) > options.max_context_bytes {
                break;
            }
            bytes += candidate_bytes;
            results.push(candidate);
        }
        Ok(results)
    }

    pub fn file(&self, path: &str) -> Option<&IndexedFile> {
        self.files.iter().find(|file| file.path == path)
    }

    pub fn context_items(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<ContextItem>, RepositoryIndexError> {
        Ok(self
            .search(query, options)?
            .into_iter()
            .map(|item| ContextItem {
                label: format!("{}:{}-{}", item.path, item.start_line, item.end_line),
                estimated_tokens: ((item.text.len() + 3) / 4) as u32,
                content: item.text,
            })
            .collect())
    }
}

fn walk_directory(
    root: &Path,
    directory: &Path,
    config: &IndexConfig,
    index: &mut RepositoryIndex,
    content_cache: &mut BTreeMap<String, String>,
    cached_bytes: &mut usize,
) -> Result<(), RepositoryIndexError> {
    if index.files.len() >= config.max_files {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| RepositoryIndexError::Io(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| RepositoryIndexError::Io(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if index.files.len() >= config.max_files {
            break;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| RepositoryIndexError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if config
                .ignored_directories
                .contains(&entry.file_name().to_string_lossy().to_string())
            {
                continue;
            }
            walk_directory(root, &path, config, index, content_cache, cached_bytes)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !config.include_extensions.contains(&extension)
            || metadata.len() > config.max_file_bytes as u64
        {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| RepositoryIndexError::Io(error.to_string()))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|error| RepositoryIndexError::Path(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let language = language_for_extension(&extension);
        let sha256 = hash_bytes(&bytes);
        index.total_bytes = index.total_bytes.saturating_add(bytes.len() as u64);
        index.files.push(IndexedFile {
            path: relative.clone(),
            language: language.clone(),
            bytes: bytes.len() as u64,
            sha256,
        });
        if let Ok(content) = String::from_utf8(bytes) {
            if index.symbols.len() < config.max_symbols {
                extract_symbols(
                    &relative,
                    &language,
                    &content,
                    config.max_symbols,
                    &mut index.symbols,
                );
            }
            if content.len() <= config.max_cached_bytes.saturating_sub(*cached_bytes) {
                *cached_bytes = cached_bytes.saturating_add(content.len());
                content_cache.insert(relative.clone(), content);
            }
        }
    }
    Ok(())
}

fn extract_symbols(
    path: &str,
    language: &str,
    content: &str,
    max_symbols: usize,
    symbols: &mut Vec<IndexedSymbol>,
) {
    for (line_index, raw_line) in content.lines().enumerate() {
        if symbols.len() >= max_symbols {
            return;
        }
        let trimmed = raw_line.trim();
        let words: Vec<&str> = trimmed
            .split_whitespace()
            .map(|word| word.trim_matches(|character: char| "({[:;=<>}),]".contains(character)))
            .collect();
        let Some((kind, position)) = words.iter().enumerate().find_map(|(index, word)| {
            let kind = match *word {
                "fn" | "def" | "function" | "func" => "function",
                "struct" | "class" | "interface" | "trait" | "enum" => "type",
                "module" | "contract" | "package" => "module",
                "const" | "let" | "var" | "type" => "binding",
                _ => return None,
            };
            Some((kind, index + 1))
        }) else {
            continue;
        };
        let Some(name) = words.get(position) else {
            continue;
        };
        let name = name.trim_matches(|character: char| "({[:;=<>}),]".contains(character));
        if name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|character| character.is_alphanumeric() || character == '_')
        {
            continue;
        }
        symbols.push(IndexedSymbol {
            path: path.into(),
            language: language.into(),
            kind: kind.into(),
            name: name.to_string(),
            line: line_index as u32 + 1,
            signature: trimmed.chars().take(240).collect(),
        });
    }
}

fn score_text(value: &str, tokens: &[String]) -> u32 {
    let lower = value.to_lowercase();
    tokens
        .iter()
        .filter(|token| lower.contains(token.as_str()))
        .count() as u32
}

fn language_for_extension(extension: &str) -> String {
    match extension {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "zig" => "zig",
        "swift" => "swift",
        "move" => "move",
        "sol" => "solidity",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "md" => "markdown",
        "txt" => "text",
        _ => "unknown",
    }
    .into()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex_digest(hasher.finalize().as_slice()))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn indexes_files_symbols_and_hashes_deterministically() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("main.rs"),
            "pub fn greet() {}\nstruct Agent {}\n",
        )
        .unwrap();
        fs::create_dir(directory.path().join("target")).unwrap();
        fs::write(
            directory.path().join("target/ignored.rs"),
            "fn ignored() {}\n",
        )
        .unwrap();
        let index = RepositoryIndex::build(directory.path(), &IndexConfig::default()).unwrap();
        assert_eq!(index.files.len(), 1);
        assert_eq!(index.files[0].path, "main.rs");
        assert!(index.symbols.iter().any(|symbol| symbol.name == "greet"));
        assert!(index.symbols.iter().any(|symbol| symbol.name == "Agent"));
        assert!(index.files[0].sha256.starts_with("sha256:"));
    }

    #[test]
    fn retrieves_ranked_context_with_hard_bounds() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("agent.rs"),
            "fn planner() {}\nfn planner_step() {}\n",
        )
        .unwrap();
        fs::write(directory.path().join("notes.md"), "planner design notes\n").unwrap();
        let index = RepositoryIndex::build(directory.path(), &IndexConfig::default()).unwrap();
        let results = index
            .search(
                "planner",
                &SearchOptions {
                    max_results: 1,
                    max_context_bytes: 100,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "agent.rs");
        assert!(results[0].score > 0);
        let context = index
            .context_items("planner", &SearchOptions::default())
            .unwrap();
        assert!(context[0].label.starts_with("agent.rs:"));
    }

    #[test]
    fn content_cache_is_bounded_and_search_falls_back_safely() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("cached.rs"), "fn cached() {}\n").unwrap();
        let cached = RepositoryIndex::build(directory.path(), &IndexConfig::default()).unwrap();
        assert_eq!(cached.content_cache.len(), 1);
        assert!(cached
            .search("cached", &SearchOptions::default())
            .unwrap()
            .iter()
            .any(|item| item.path == "cached.rs"));

        let uncached = RepositoryIndex::build(
            directory.path(),
            &IndexConfig {
                max_cached_bytes: 0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(uncached.content_cache.is_empty());
        assert!(uncached
            .search("cached", &SearchOptions::default())
            .unwrap()
            .iter()
            .any(|item| item.path == "cached.rs"));
    }

    #[test]
    fn skips_symlinks_and_large_files() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("small.rs"), "fn okay() {}\n").unwrap();
        fs::write(directory.path().join("large.rs"), "x".repeat(2_000)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            directory.path().join("small.rs"),
            directory.path().join("link.rs"),
        )
        .unwrap();
        let config = IndexConfig {
            max_file_bytes: 100,
            ..Default::default()
        };
        let index = RepositoryIndex::build(directory.path(), &config).unwrap();
        assert_eq!(index.files.len(), 1);
        assert_eq!(index.files[0].path, "small.rs");
    }
}
