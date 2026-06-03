//! Tree-sitter backed source extraction for Pi semantic code graphs.
//!
//! This crate owns language-specific parsing. Callers keep policy, storage, and
//! graph-shaping decisions outside the extractor layer.

#![forbid(unsafe_code)]
#![allow(clippy::missing_const_for_fn)]

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tree_sitter::{Language, Node as TreeSitterNode, Parser as TreeSitterParser};

pub const PI_CODING_DIR: &str = ".pi-coding";
pub const CODEGRAPH_DB_FILE: &str = "db.sqlite";
pub const CODEGRAPH_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCodeGraph {
    pub language_id: String,
    pub symbols: Vec<ExtractedCodeSymbol>,
    pub calls: Vec<ExtractedCodeCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCodeSymbol {
    pub kind: String,
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCodeCall {
    pub caller: String,
    pub callee: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedFile {
    pub source_path: String,
    pub language_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub mtime_unix_ns: Option<u64>,
    pub symbol_count: usize,
    pub call_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeGraphSymbol {
    pub source_path: String,
    pub language_id: String,
    pub kind: String,
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeGraphCall {
    pub source_path: String,
    pub language_id: String,
    pub caller: String,
    pub callee: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeGraphNode {
    pub symbol: CodeGraphSymbol,
    pub callers: Vec<CodeGraphCall>,
    pub callees: Vec<CodeGraphCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeGraphTrace {
    pub from: String,
    pub to: String,
    pub path: Vec<CodeGraphCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeGraphImpact {
    pub symbol: String,
    pub depth: usize,
    pub impacted_symbols: Vec<CodeGraphSymbol>,
    pub paths: Vec<CodeGraphTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub db_path: PathBuf,
    pub scanned_files: usize,
    pub indexed_files: usize,
    pub unchanged_files: usize,
    pub removed_files: usize,
    pub skipped_files: usize,
}

#[derive(Debug)]
pub enum CodeGraphError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    RootNotDirectory(PathBuf),
    InvalidProjectPath(PathBuf),
    IndexNotInitialized(PathBuf),
}

impl fmt::Display for CodeGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::RootNotDirectory(path) => {
                write!(
                    formatter,
                    "project root is not a directory: {}",
                    path.display()
                )
            }
            Self::InvalidProjectPath(path) => {
                write!(formatter, "invalid project path: {}", path.display())
            }
            Self::IndexNotInitialized(path) => {
                write!(
                    formatter,
                    "codegraph index is not initialized: {}",
                    path.display()
                )
            }
        }
    }
}

impl StdError for CodeGraphError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::RootNotDirectory(_)
            | Self::InvalidProjectPath(_)
            | Self::IndexNotInitialized(_) => None,
        }
    }
}

impl From<io::Error> for CodeGraphError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for CodeGraphError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

pub type CodeGraphResult<T> = Result<T, CodeGraphError>;

#[derive(Debug, Clone)]
pub struct CodeGraphIndex {
    root: PathBuf,
    db_path: PathBuf,
}

impl CodeGraphIndex {
    pub fn open(project_root: impl Into<PathBuf>) -> CodeGraphResult<Self> {
        let root = project_root.into();
        if !root.is_dir() {
            return Err(CodeGraphError::RootNotDirectory(root));
        }
        let db_path = project_db_path(&root);
        let index = Self { root, db_path };
        index.ensure_database()?;
        Ok(index)
    }

    pub fn open_existing(project_root: impl Into<PathBuf>) -> CodeGraphResult<Self> {
        let root = project_root.into();
        if !root.is_dir() {
            return Err(CodeGraphError::RootNotDirectory(root));
        }
        let db_path = project_db_path(&root);
        if !db_path.exists() {
            return Err(CodeGraphError::IndexNotInitialized(db_path));
        }
        Ok(Self { root, db_path })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn sync_project(&self) -> CodeGraphResult<SyncReport> {
        let files = discover_supported_files(&self.root)?;
        self.sync_discovered_files(files, true)
    }

    pub fn sync_paths(&self, paths: &[PathBuf]) -> CodeGraphResult<SyncReport> {
        let mut files = Vec::new();
        for path in paths {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                self.root.join(path)
            };
            if absolute.is_file() {
                if supported_source_path(&normalize_relative_path(&self.root, &absolute)) {
                    files.push(absolute);
                }
            } else if absolute.is_dir() {
                files.extend(discover_supported_files(&absolute)?);
            } else {
                self.remove_path(path)?;
            }
        }
        self.sync_discovered_files(files, false)
    }

    pub fn indexed_files(&self) -> CodeGraphResult<Vec<IndexedFile>> {
        let conn = self.open_connection()?;
        let mut stmt = conn.prepare(
            "SELECT source_path, language_id, sha256, size_bytes, mtime_unix_ns,
                    symbol_count, call_count
             FROM files
             ORDER BY source_path",
        )?;
        let files = stmt
            .query_map([], indexed_file_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(files)
    }

    pub fn search(&self, query: &str, limit: usize) -> CodeGraphResult<Vec<CodeGraphSymbol>> {
        let conn = self.open_connection()?;
        let pattern = format!("%{}%", escape_like(query));
        let max_rows = bounded_limit(limit);
        let mut stmt = conn.prepare(
            "SELECT source_path, language_id, kind, name, line_start, line_end, is_test
             FROM symbols
             WHERE name LIKE ?1 ESCAPE '\\'
                OR kind LIKE ?1 ESCAPE '\\'
                OR source_path LIKE ?1 ESCAPE '\\'
             ORDER BY
                CASE WHEN name = ?2 THEN 0
                     WHEN name LIKE ?1 ESCAPE '\\' THEN 1
                     ELSE 2
                END,
                source_path,
                line_start
             LIMIT ?3",
        )?;
        let symbols = stmt
            .query_map(params![pattern, query, max_rows], symbol_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(symbols)
    }

    pub fn callers(&self, symbol: &str, limit: usize) -> CodeGraphResult<Vec<CodeGraphCall>> {
        self.calls_by_column("callee", symbol, limit)
    }

    pub fn callees(&self, symbol: &str, limit: usize) -> CodeGraphResult<Vec<CodeGraphCall>> {
        self.calls_by_column("caller", symbol, limit)
    }

    pub fn node(&self, symbol: &str) -> CodeGraphResult<Option<CodeGraphNode>> {
        let conn = self.open_connection()?;
        let mut stmt = conn.prepare(
            "SELECT source_path, language_id, kind, name, line_start, line_end, is_test
             FROM symbols
             WHERE name = ?1
             ORDER BY source_path, line_start
             LIMIT 1",
        )?;
        let symbol = stmt
            .query_row(params![symbol], symbol_from_row)
            .optional()?;
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        let incoming = self.callers(&symbol.name, 100)?;
        let outgoing = self.callees(&symbol.name, 100)?;
        Ok(Some(CodeGraphNode {
            symbol,
            callers: incoming,
            callees: outgoing,
        }))
    }

    pub fn impact(&self, symbol: &str, depth: usize) -> CodeGraphResult<CodeGraphImpact> {
        let max_depth = depth.clamp(1, 8);
        let call_graph = self.load_calls()?;
        let symbols = self.symbols_by_name()?;
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        let mut paths = Vec::new();
        visited.insert(symbol.to_string());
        queue.push_back((symbol.to_string(), Vec::<CodeGraphCall>::new(), 0usize));

        while let Some((current, path, current_depth)) = queue.pop_front() {
            if current_depth >= max_depth {
                continue;
            }
            for call in call_graph.callers_of(&current) {
                let next = call.caller.clone();
                let mut next_path = path.clone();
                next_path.push(call.clone());
                paths.push(CodeGraphTrace {
                    from: next.clone(),
                    to: symbol.to_string(),
                    path: next_path.clone(),
                });
                if visited.insert(next.clone()) {
                    queue.push_back((next, next_path, current_depth + 1));
                }
            }
        }

        let impacted_symbols = visited
            .iter()
            .filter(|name| name.as_str() != symbol)
            .filter_map(|name| symbols.get(name).cloned())
            .collect();
        Ok(CodeGraphImpact {
            symbol: symbol.to_string(),
            depth: max_depth,
            impacted_symbols,
            paths,
        })
    }

    pub fn trace(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
    ) -> CodeGraphResult<Option<CodeGraphTrace>> {
        let max_depth = max_depth.clamp(1, 16);
        let call_graph = self.load_calls()?;
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        visited.insert(from.to_string());
        queue.push_back((from.to_string(), Vec::<CodeGraphCall>::new()));

        while let Some((current, path)) = queue.pop_front() {
            if path.len() >= max_depth {
                continue;
            }
            for call in call_graph.callees_of(&current) {
                let next = call.callee.clone();
                let mut next_path = path.clone();
                next_path.push(call.clone());
                if next == to {
                    return Ok(Some(CodeGraphTrace {
                        from: from.to_string(),
                        to: to.to_string(),
                        path: next_path,
                    }));
                }
                if visited.insert(next.clone()) {
                    queue.push_back((next, next_path));
                }
            }
        }

        Ok(None)
    }

    fn sync_discovered_files(
        &self,
        mut absolute_paths: Vec<PathBuf>,
        prune_missing: bool,
    ) -> CodeGraphResult<SyncReport> {
        absolute_paths.sort_by_key(|path| normalize_path(path));
        absolute_paths.dedup();
        let mut conn = self.open_or_create_connection()?;
        let tx = conn.transaction()?;
        let mut report = SyncReport {
            db_path: self.db_path.clone(),
            scanned_files: absolute_paths.len(),
            indexed_files: 0,
            unchanged_files: 0,
            removed_files: 0,
            skipped_files: 0,
        };
        let mut seen_paths = BTreeSet::new();

        for absolute_path in absolute_paths {
            let source_path = normalize_relative_path(&self.root, &absolute_path);
            seen_paths.insert(source_path.clone());
            match index_file(&tx, &absolute_path, &source_path)? {
                FileIndexOutcome::Indexed => report.indexed_files += 1,
                FileIndexOutcome::Unchanged => report.unchanged_files += 1,
                FileIndexOutcome::Skipped => report.skipped_files += 1,
            }
        }

        if prune_missing {
            report.removed_files = prune_deleted_files(&tx, &seen_paths)?;
        }
        set_meta(&tx, "schema_version", &CODEGRAPH_SCHEMA_VERSION.to_string())?;
        tx.commit()?;
        Ok(report)
    }

    fn remove_path(&self, path: &Path) -> CodeGraphResult<()> {
        let source_path = if path.is_absolute() {
            normalize_relative_path(&self.root, path)
        } else {
            normalize_path(path)
        };
        let conn = self.open_or_create_connection()?;
        conn.execute(
            "DELETE FROM files WHERE source_path = ?1",
            params![source_path],
        )?;
        Ok(())
    }

    fn ensure_database(&self) -> CodeGraphResult<()> {
        let parent = self
            .db_path
            .parent()
            .ok_or_else(|| CodeGraphError::InvalidProjectPath(self.db_path.clone()))?;
        fs::create_dir_all(parent)?;
        let conn = self.open_or_create_connection()?;
        initialize_schema(&conn)?;
        Ok(())
    }

    fn open_connection(&self) -> CodeGraphResult<Connection> {
        Ok(Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?)
    }

    fn open_or_create_connection(&self) -> CodeGraphResult<Connection> {
        Ok(Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?)
    }

    fn calls_by_column(
        &self,
        column: &str,
        symbol: &str,
        limit: usize,
    ) -> CodeGraphResult<Vec<CodeGraphCall>> {
        let conn = self.open_connection()?;
        let sql = match column {
            "caller" => {
                "SELECT source_path, language_id, caller, callee, line
                 FROM calls
                 WHERE caller = ?1
                 ORDER BY source_path, line
                 LIMIT ?2"
            }
            "callee" => {
                "SELECT source_path, language_id, caller, callee, line
                 FROM calls
                 WHERE callee = ?1
                 ORDER BY source_path, line
                 LIMIT ?2"
            }
            _ => unreachable!("invalid calls column"),
        };
        let mut stmt = conn.prepare(sql)?;
        let calls = stmt
            .query_map(params![symbol, bounded_limit(limit)], call_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(calls)
    }

    fn load_calls(&self) -> CodeGraphResult<CallGraph> {
        let conn = self.open_connection()?;
        let mut stmt = conn.prepare(
            "SELECT source_path, language_id, caller, callee, line
             FROM calls
             ORDER BY source_path, line",
        )?;
        let calls = stmt
            .query_map([], call_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CallGraph::new(calls))
    }

    fn symbols_by_name(&self) -> CodeGraphResult<BTreeMap<String, CodeGraphSymbol>> {
        let conn = self.open_connection()?;
        let mut stmt = conn.prepare(
            "SELECT source_path, language_id, kind, name, line_start, line_end, is_test
             FROM symbols
             ORDER BY source_path, line_start",
        )?;
        let mut symbols = BTreeMap::new();
        for symbol in stmt.query_map([], symbol_from_row)? {
            let symbol = symbol?;
            symbols.entry(symbol.name.clone()).or_insert(symbol);
        }
        Ok(symbols)
    }
}

#[must_use]
pub fn project_db_path(project_root: &Path) -> PathBuf {
    project_root.join(PI_CODING_DIR).join(CODEGRAPH_DB_FILE)
}

fn initialize_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS files (
            source_path TEXT PRIMARY KEY,
            language_id TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            mtime_unix_ns INTEGER,
            symbol_count INTEGER NOT NULL,
            call_count INTEGER NOT NULL,
            indexed_at_unix_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_path TEXT NOT NULL REFERENCES files(source_path) ON DELETE CASCADE,
            language_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            is_test INTEGER NOT NULL,
            UNIQUE(source_path, kind, name, line_start)
        );
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_path ON symbols(source_path);
        CREATE TABLE IF NOT EXISTS calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_path TEXT NOT NULL REFERENCES files(source_path) ON DELETE CASCADE,
            language_id TEXT NOT NULL,
            caller TEXT NOT NULL,
            callee TEXT NOT NULL,
            line INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_calls_callee ON calls(callee);
        CREATE INDEX IF NOT EXISTS idx_calls_caller ON calls(caller);
        CREATE INDEX IF NOT EXISTS idx_calls_path ON calls(source_path);
        ",
    )?;
    set_meta(
        conn,
        "schema_version",
        &CODEGRAPH_SCHEMA_VERSION.to_string(),
    )?;
    Ok(())
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn discover_supported_files(root: &Path) -> CodeGraphResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_supported_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_supported_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
) -> CodeGraphResult<()> {
    let entries = fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_supported_files(root, &path, files)?;
        } else if path.is_file() && supported_source_path(&normalize_relative_path(root, &path)) {
            files.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | "target" | PI_CODING_DIR))
}

fn supported_source_path(source_path: &str) -> bool {
    extractor_for_path(source_path).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileIndexOutcome {
    Indexed,
    Unchanged,
    Skipped,
}

fn index_file(
    conn: &Connection,
    absolute_path: &Path,
    source_path: &str,
) -> CodeGraphResult<FileIndexOutcome> {
    let bytes = fs::read(absolute_path)?;
    let sha256 = sha256_hex(&bytes);
    let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let mtime_unix_ns = file_mtime_unix_ns(absolute_path)?;
    if existing_file_sha(conn, source_path)?.is_some_and(|existing| existing == sha256) {
        return Ok(FileIndexOutcome::Unchanged);
    }

    let content = String::from_utf8_lossy(&bytes);
    let Some(graph) = extract_code_graph(source_path, &content) else {
        return Ok(FileIndexOutcome::Skipped);
    };
    conn.execute(
        "INSERT INTO files(
            source_path, language_id, sha256, size_bytes, mtime_unix_ns,
            symbol_count, call_count, indexed_at_unix_ms
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(source_path) DO UPDATE SET
            language_id = excluded.language_id,
            sha256 = excluded.sha256,
            size_bytes = excluded.size_bytes,
            mtime_unix_ns = excluded.mtime_unix_ns,
            symbol_count = excluded.symbol_count,
            call_count = excluded.call_count,
            indexed_at_unix_ms = excluded.indexed_at_unix_ms",
        params![
            source_path,
            graph.language_id.as_str(),
            sha256,
            i64::try_from(size_bytes).unwrap_or(i64::MAX),
            mtime_unix_ns.and_then(|value| i64::try_from(value).ok()),
            i64::try_from(graph.symbols.len()).unwrap_or(i64::MAX),
            i64::try_from(graph.calls.len()).unwrap_or(i64::MAX),
            current_unix_ms(),
        ],
    )?;
    conn.execute(
        "DELETE FROM symbols WHERE source_path = ?1",
        params![source_path],
    )?;
    conn.execute(
        "DELETE FROM calls WHERE source_path = ?1",
        params![source_path],
    )?;
    for symbol in &graph.symbols {
        conn.execute(
            "INSERT OR REPLACE INTO symbols(
                source_path, language_id, kind, name, line_start, line_end, is_test
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                source_path,
                graph.language_id.as_str(),
                symbol.kind.as_str(),
                symbol.name.as_str(),
                i64::try_from(symbol.line_start).unwrap_or(i64::MAX),
                i64::try_from(symbol.line_end).unwrap_or(i64::MAX),
                symbol.is_test,
            ],
        )?;
    }
    for call in &graph.calls {
        conn.execute(
            "INSERT INTO calls(source_path, language_id, caller, callee, line)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                source_path,
                graph.language_id.as_str(),
                call.caller.as_str(),
                call.callee.as_str(),
                i64::try_from(call.line).unwrap_or(i64::MAX),
            ],
        )?;
    }
    Ok(FileIndexOutcome::Indexed)
}

fn existing_file_sha(conn: &Connection, source_path: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT sha256 FROM files WHERE source_path = ?1",
        params![source_path],
        |row| row.get(0),
    )
    .optional()
}

fn prune_deleted_files(conn: &Connection, seen_paths: &BTreeSet<String>) -> CodeGraphResult<usize> {
    let mut stmt = conn.prepare("SELECT source_path FROM files ORDER BY source_path")?;
    let indexed_paths = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut removed = 0;
    for source_path in indexed_paths {
        if !seen_paths.contains(&source_path) {
            conn.execute(
                "DELETE FROM files WHERE source_path = ?1",
                params![source_path],
            )?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn indexed_file_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedFile> {
    let size_bytes: i64 = row.get(3)?;
    let mtime_unix_ns: Option<i64> = row.get(4)?;
    let symbol_count: i64 = row.get(5)?;
    let call_count: i64 = row.get(6)?;
    Ok(IndexedFile {
        source_path: row.get(0)?,
        language_id: row.get(1)?,
        sha256: row.get(2)?,
        size_bytes: u64::try_from(size_bytes).unwrap_or_default(),
        mtime_unix_ns: mtime_unix_ns.and_then(|value| u64::try_from(value).ok()),
        symbol_count: usize::try_from(symbol_count).unwrap_or_default(),
        call_count: usize::try_from(call_count).unwrap_or_default(),
    })
}

#[derive(Debug, Clone)]
struct CallGraph {
    by_caller: BTreeMap<String, Vec<CodeGraphCall>>,
    by_callee: BTreeMap<String, Vec<CodeGraphCall>>,
}

impl CallGraph {
    fn new(calls: Vec<CodeGraphCall>) -> Self {
        let mut outgoing: BTreeMap<String, Vec<CodeGraphCall>> = BTreeMap::new();
        let mut incoming: BTreeMap<String, Vec<CodeGraphCall>> = BTreeMap::new();
        for call in calls {
            outgoing
                .entry(call.caller.clone())
                .or_default()
                .push(call.clone());
            incoming.entry(call.callee.clone()).or_default().push(call);
        }
        Self {
            by_caller: outgoing,
            by_callee: incoming,
        }
    }

    fn callees_of(&self, symbol: &str) -> &[CodeGraphCall] {
        self.by_caller.get(symbol).map_or(&[], Vec::as_slice)
    }

    fn callers_of(&self, symbol: &str) -> &[CodeGraphCall] {
        self.by_callee.get(symbol).map_or(&[], Vec::as_slice)
    }
}

fn symbol_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeGraphSymbol> {
    let line_start: i64 = row.get(4)?;
    let line_end: i64 = row.get(5)?;
    Ok(CodeGraphSymbol {
        source_path: row.get(0)?,
        language_id: row.get(1)?,
        kind: row.get(2)?,
        name: row.get(3)?,
        line_start: usize::try_from(line_start).unwrap_or_default(),
        line_end: usize::try_from(line_end).unwrap_or_default(),
        is_test: row.get(6)?,
    })
}

fn call_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeGraphCall> {
    let line: i64 = row.get(4)?;
    Ok(CodeGraphCall {
        source_path: row.get(0)?,
        language_id: row.get(1)?,
        caller: row.get(2)?,
        callee: row.get(3)?,
        line: usize::try_from(line).unwrap_or_default(),
    })
}

fn bounded_limit(limit: usize) -> i64 {
    i64::try_from(limit.clamp(1, 500)).unwrap_or(500)
}

fn escape_like(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len());
    for ch in query.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn file_mtime_unix_ns(path: &Path) -> CodeGraphResult<Option<u64>> {
    let metadata = fs::metadata(path)?;
    let Ok(modified) = metadata.modified() else {
        return Ok(None);
    };
    let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
        return Ok(None);
    };
    Ok(duration
        .as_secs()
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(u64::from(duration.subsec_nanos()))))
}

fn current_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn normalize_relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    normalize_path(relative)
}

fn normalize_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str().map(ToString::to_string),
            std::path::Component::CurDir => None,
            std::path::Component::ParentDir => Some("..".to_string()),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                Some(component.as_os_str().to_string_lossy().to_string())
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub trait CodeLanguageExtractor: Sync {
    fn language_id(&self) -> &'static str;
    fn supports_path(&self, source_path: &str) -> bool;
    fn extract(&self, source_path: &str, content: &str) -> Option<ExtractedCodeGraph>;
}

static RUST_TREE_SITTER_EXTRACTOR: RustTreeSitterExtractor = RustTreeSitterExtractor;
static GO_TREE_SITTER_EXTRACTOR: GoTreeSitterExtractor = GoTreeSitterExtractor;

static EXTRACTORS: [&dyn CodeLanguageExtractor; 2] =
    [&RUST_TREE_SITTER_EXTRACTOR, &GO_TREE_SITTER_EXTRACTOR];

#[must_use]
pub fn extractor_for_path(source_path: &str) -> Option<&'static dyn CodeLanguageExtractor> {
    EXTRACTORS
        .iter()
        .copied()
        .find(|extractor| extractor.supports_path(source_path))
}

#[must_use]
pub fn extract_code_graph(source_path: &str, content: &str) -> Option<ExtractedCodeGraph> {
    extractor_for_path(source_path)?.extract(source_path, content)
}

#[derive(Debug, Clone, Copy)]
pub struct RustTreeSitterExtractor;

impl CodeLanguageExtractor for RustTreeSitterExtractor {
    fn language_id(&self) -> &'static str {
        "rust"
    }

    fn supports_path(&self, source_path: &str) -> bool {
        has_extension(source_path, "rs")
    }

    fn extract(&self, source_path: &str, content: &str) -> Option<ExtractedCodeGraph> {
        if !self.supports_path(source_path) {
            return None;
        }
        parse_tree_sitter_ast(
            self.language_id(),
            &tree_sitter_rust::LANGUAGE.into(),
            content,
            collect_rust_ast_symbols,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GoTreeSitterExtractor;

impl CodeLanguageExtractor for GoTreeSitterExtractor {
    fn language_id(&self) -> &'static str {
        "go"
    }

    fn supports_path(&self, source_path: &str) -> bool {
        has_extension(source_path, "go")
    }

    fn extract(&self, source_path: &str, content: &str) -> Option<ExtractedCodeGraph> {
        if !self.supports_path(source_path) {
            return None;
        }
        parse_tree_sitter_ast(
            self.language_id(),
            &tree_sitter_go::LANGUAGE.into(),
            content,
            collect_go_ast_symbols,
        )
    }
}

type AstCollector = fn(
    TreeSitterNode<'_>,
    &[u8],
    &mut Vec<ExtractedCodeSymbol>,
    &mut Vec<ExtractedCodeCall>,
    Option<&str>,
    bool,
);

fn parse_tree_sitter_ast(
    language_id: &str,
    language: &Language,
    content: &str,
    collect: AstCollector,
) -> Option<ExtractedCodeGraph> {
    let mut parser = TreeSitterParser::new();
    parser.set_language(language).ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }

    let bytes = content.as_bytes();
    let mut symbols = Vec::new();
    let mut calls = Vec::new();
    collect(root, bytes, &mut symbols, &mut calls, None, false);
    Some(normalize_extraction(language_id, symbols, calls))
}

fn normalize_extraction(
    language_id: &str,
    mut symbols: Vec<ExtractedCodeSymbol>,
    mut calls: Vec<ExtractedCodeCall>,
) -> ExtractedCodeGraph {
    symbols.sort_by(|left, right| {
        left.line_start
            .cmp(&right.line_start)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });
    symbols.dedup_by(|left, right| {
        left.kind == right.kind && left.name == right.name && left.line_start == right.line_start
    });
    calls.sort_by(|left, right| {
        left.caller
            .cmp(&right.caller)
            .then_with(|| left.callee.cmp(&right.callee))
            .then_with(|| left.line.cmp(&right.line))
    });
    calls.dedup();
    ExtractedCodeGraph {
        language_id: language_id.to_string(),
        symbols,
        calls,
    }
}

fn collect_rust_ast_symbols(
    node: TreeSitterNode<'_>,
    bytes: &[u8],
    symbols: &mut Vec<ExtractedCodeSymbol>,
    calls: &mut Vec<ExtractedCodeCall>,
    current_symbol: Option<&str>,
    pending_test_attribute: bool,
) {
    let mut cursor = node.walk();
    let mut next_test_attribute = pending_test_attribute;
    for child in node.named_children(&mut cursor) {
        if is_rust_test_attribute_node(child, bytes) {
            next_test_attribute = true;
            continue;
        }

        if let Some(symbol) = rust_symbol_from_node(child, bytes, next_test_attribute) {
            let symbol_name = symbol.name.clone();
            let scan_calls = matches!(symbol.kind.as_str(), "fn" | "trait_fn");
            symbols.push(symbol);
            if scan_calls {
                collect_rust_ast_symbols(child, bytes, symbols, calls, Some(&symbol_name), false);
            } else {
                collect_rust_ast_symbols(child, bytes, symbols, calls, None, false);
            }
            next_test_attribute = false;
            continue;
        }

        if let Some(caller) = current_symbol
            && let Some(callee) = rust_call_name_from_node(child, bytes)
        {
            calls.push(ExtractedCodeCall {
                caller: caller.to_string(),
                callee,
                line: one_indexed_row(child),
            });
        }

        collect_rust_ast_symbols(child, bytes, symbols, calls, current_symbol, false);
        next_test_attribute = false;
    }
}

fn rust_symbol_from_node(
    node: TreeSitterNode<'_>,
    bytes: &[u8],
    is_test: bool,
) -> Option<ExtractedCodeSymbol> {
    let kind = match node.kind() {
        "function_item" => "fn",
        "function_signature_item" => "trait_fn",
        "struct_item" => "struct",
        "enum_item" => "enum",
        "trait_item" => "trait",
        "impl_item" => "impl",
        "mod_item" => "mod",
        "type_item" => "type",
        "const_item" => "const",
        "static_item" => "static",
        _ => return None,
    };
    let name = if node.kind() == "impl_item" {
        rust_impl_name(node, bytes)?
    } else {
        node.child_by_field_name("name")
            .and_then(|name| node_text(name, bytes))
            .map(ToString::to_string)?
    };
    Some(ExtractedCodeSymbol {
        kind: kind.to_string(),
        name,
        line_start: one_indexed_row(node),
        line_end: node.end_position().row.saturating_add(1),
        is_test: is_test || rust_node_has_test_attribute(node, bytes),
    })
}

fn rust_node_has_test_attribute(node: TreeSitterNode<'_>, bytes: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| is_rust_test_attribute_node(child, bytes))
}

fn rust_impl_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("type")
        .and_then(|node| node_text(node, bytes))
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .filter_map(|child| node_text(child, bytes))
                .find(|text| !matches!(*text, "impl" | "for"))
        })
        .map(|text| format!("impl {}", collapse_ws(text)))
}

fn rust_call_name_from_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            rust_callable_name(function, bytes)
        }
        "macro_invocation" => node
            .child_by_field_name("macro")
            .and_then(|macro_node| node_text(macro_node, bytes))
            .map(|name| format!("{name}!")),
        _ => None,
    }
}

fn rust_callable_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, bytes).map(ToString::to_string),
        "scoped_identifier" => node_text(node, bytes)
            .and_then(|text| text.rsplit("::").next())
            .filter(|name| !name.is_empty())
            .map(ToString::to_string),
        "generic_function" => node
            .child_by_field_name("function")
            .and_then(|function| rust_callable_name(function, bytes)),
        "field_expression" => node
            .child_by_field_name("field")
            .and_then(|field| node_text(field, bytes))
            .map(ToString::to_string),
        _ => node_text(node, bytes).map(collapse_ws),
    }
}

fn is_rust_test_attribute_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> bool {
    if !matches!(node.kind(), "attribute_item" | "inner_attribute_item") {
        return false;
    }
    node_text(node, bytes).is_some_and(|text| {
        let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
        compact == "#[test]"
            || compact.starts_with("#[tokio::test")
            || compact.starts_with("#[asupersync::test")
            || compact.starts_with("#[should_panic")
    })
}

fn collect_go_ast_symbols(
    node: TreeSitterNode<'_>,
    bytes: &[u8],
    symbols: &mut Vec<ExtractedCodeSymbol>,
    calls: &mut Vec<ExtractedCodeCall>,
    current_symbol: Option<&str>,
    _pending_test_attribute: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(symbol) = go_symbol_from_node(child, bytes) {
            let symbol_name = symbol.name.clone();
            let scan_calls = matches!(symbol.kind.as_str(), "func" | "method");
            symbols.push(symbol);
            if scan_calls {
                collect_go_ast_symbols(child, bytes, symbols, calls, Some(&symbol_name), false);
            } else {
                collect_go_ast_symbols(child, bytes, symbols, calls, None, false);
            }
            continue;
        }

        if let Some(caller) = current_symbol
            && let Some(callee) = go_call_name_from_node(child, bytes)
        {
            calls.push(ExtractedCodeCall {
                caller: caller.to_string(),
                callee,
                line: one_indexed_row(child),
            });
        }

        collect_go_ast_symbols(child, bytes, symbols, calls, current_symbol, false);
    }
}

fn go_symbol_from_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<ExtractedCodeSymbol> {
    let (kind, name) = match node.kind() {
        "function_declaration" => (
            "func",
            node.child_by_field_name("name")
                .and_then(|name| node_text(name, bytes))
                .map(ToString::to_string)?,
        ),
        "method_declaration" => ("method", go_method_name(node, bytes)?),
        "type_declaration" => ("type", go_type_declaration_name(node, bytes)?),
        _ => return None,
    };
    Some(ExtractedCodeSymbol {
        kind: kind.to_string(),
        is_test: is_go_test_symbol(kind, &name),
        name,
        line_start: one_indexed_row(node),
        line_end: node.end_position().row.saturating_add(1),
    })
}

fn go_method_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    let name = node
        .child_by_field_name("name")
        .and_then(|name| node_text(name, bytes))?;
    let receiver = node
        .child_by_field_name("receiver")
        .and_then(|receiver| node_text(receiver, bytes))
        .map(go_receiver_type_name);
    receiver.map_or_else(
        || Some(name.to_string()),
        |receiver| Some(format!("{receiver}.{name}")),
    )
}

fn go_receiver_type_name(receiver: &str) -> String {
    let trimmed = receiver
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let tokens: Vec<&str> = trimmed
        .split(|ch: char| {
            !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '*' || ch == '[' || ch == ']')
        })
        .filter(|token| !token.is_empty())
        .collect();
    tokens.last().map_or_else(
        || collapse_ws(trimmed),
        |token| token.trim_start_matches('*').to_string(),
    )
}

fn go_type_declaration_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| match child.kind() {
            "type_spec" => child
                .child_by_field_name("name")
                .and_then(|name| node_text(name, bytes))
                .map(ToString::to_string),
            _ => None,
        })
}

fn is_go_test_symbol(kind: &str, name: &str) -> bool {
    kind == "func"
        && (name.starts_with("Test")
            || name.starts_with("Benchmark")
            || name.starts_with("Example"))
}

fn go_call_name_from_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    go_callable_name(function, bytes)
}

fn go_callable_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, bytes).map(ToString::to_string),
        "selector_expression" => node
            .child_by_field_name("field")
            .and_then(|field| node_text(field, bytes))
            .map(ToString::to_string)
            .or_else(|| {
                node_text(node, bytes)
                    .and_then(|text| text.rsplit('.').next())
                    .filter(|name| !name.is_empty())
                    .map(ToString::to_string)
            }),
        _ => node_text(node, bytes).map(collapse_ws),
    }
}

fn node_text<'a>(node: TreeSitterNode<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(bytes).ok()
}

fn one_indexed_row(node: TreeSitterNode<'_>) -> usize {
    node.start_position().row.saturating_add(1)
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_extension(source_path: &str, extension: &str) -> bool {
    Path::new(source_path)
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
mod tests {
    use super::{CodeGraphIndex, ExtractedCodeCall, extract_code_graph};

    #[test]
    fn rust_extractor_indexes_symbols_and_calls() {
        let graph = extract_code_graph(
            "src/lib.rs",
            r"
                struct Agent;

                #[test]
                fn smoke() {
                    helper();
                    value.render();
                }
            ",
        )
        .expect("rust graph");

        assert_eq!(graph.language_id, "rust");
        assert!(graph.symbols.iter().any(|symbol| {
            symbol.kind == "struct" && symbol.name == "Agent" && !symbol.is_test
        }));
        assert!(
            graph
                .symbols
                .iter()
                .any(|symbol| { symbol.kind == "fn" && symbol.name == "smoke" && symbol.is_test })
        );
        assert_has_call(&graph.calls, "smoke", "helper");
        assert_has_call(&graph.calls, "smoke", "render");
    }

    #[test]
    fn go_extractor_indexes_symbols_and_calls() {
        let graph = extract_code_graph(
            "src/server.go",
            r"
                package server

                type Agent struct{}

                func helper() {}

                func TestSmoke(t *testing.T) {
                    helper()
                    value.Render()
                }

                func (a *Agent) Run() {
                    helper()
                }
            ",
        )
        .expect("go graph");

        assert_eq!(graph.language_id, "go");
        assert!(
            graph
                .symbols
                .iter()
                .any(|symbol| symbol.kind == "type" && symbol.name == "Agent")
        );
        assert!(graph.symbols.iter().any(|symbol| {
            symbol.kind == "func" && symbol.name == "TestSmoke" && symbol.is_test
        }));
        assert!(
            graph
                .symbols
                .iter()
                .any(|symbol| symbol.kind == "method" && symbol.name == "Agent.Run")
        );
        assert_has_call(&graph.calls, "TestSmoke", "helper");
        assert_has_call(&graph.calls, "TestSmoke", "Render");
        assert_has_call(&graph.calls, "Agent.Run", "helper");
    }

    #[test]
    fn project_index_persists_supported_files_and_skips_unchanged_syncs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create src");
        std::fs::write(
            src.join("lib.rs"),
            r"
                fn helper() {}

                fn caller() {
                    helper();
                }
            ",
        )
        .expect("write rust source");

        let index = CodeGraphIndex::open(dir.path()).expect("open index");
        assert!(index.db_path().ends_with(".pi-coding/db.sqlite"));

        let first = index.sync_project().expect("first sync");
        assert_eq!(first.scanned_files, 1);
        assert_eq!(first.indexed_files, 1);
        assert_eq!(first.unchanged_files, 0);
        assert!(index.db_path().exists());

        let files = index.indexed_files().expect("indexed files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].source_path, "src/lib.rs");
        assert_eq!(files[0].language_id, "rust");
        assert_eq!(files[0].symbol_count, 2);
        assert_eq!(files[0].call_count, 1);

        let second = index.sync_project().expect("second sync");
        assert_eq!(second.indexed_files, 0);
        assert_eq!(second.unchanged_files, 1);
    }

    #[test]
    fn open_existing_does_not_create_uninitialized_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = super::project_db_path(dir.path());
        let err = CodeGraphIndex::open_existing(dir.path()).expect_err("missing index");
        assert!(matches!(err, super::CodeGraphError::IndexNotInitialized(_)));
        assert!(!db_path.exists());
    }

    #[test]
    fn project_index_prunes_deleted_files_on_full_sync() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create src");
        let file = src.join("main.go");
        std::fs::write(&file, "package main\nfunc main() {}\n").expect("write go source");

        let index = CodeGraphIndex::open(dir.path()).expect("open index");
        let first = index.sync_project().expect("first sync");
        assert_eq!(first.indexed_files, 1);

        std::fs::remove_file(&file).expect("remove test file");
        let second = index.sync_project().expect("second sync");
        assert_eq!(second.removed_files, 1);
        assert!(index.indexed_files().expect("indexed files").is_empty());
    }

    #[test]
    fn project_index_answers_codegraph_queries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create src");
        std::fs::write(
            src.join("lib.rs"),
            r"
                fn leaf() {}

                fn middle() {
                    leaf();
                }

                fn root() {
                    middle();
                }
            ",
        )
        .expect("write rust source");

        let index = CodeGraphIndex::open(dir.path()).expect("open index");
        index.sync_project().expect("sync");

        let search = index.search("mid", 10).expect("search");
        assert!(search.iter().any(|symbol| symbol.name == "middle"));

        let incoming = index.callers("middle", 10).expect("callers");
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].caller, "root");

        let outgoing = index.callees("middle", 10).expect("callees");
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].callee, "leaf");

        let node = index.node("middle").expect("node").expect("middle node");
        assert_eq!(node.symbol.name, "middle");
        assert_eq!(node.callers[0].caller, "root");
        assert_eq!(node.callees[0].callee, "leaf");

        let impact = index.impact("leaf", 4).expect("impact");
        assert!(
            impact
                .impacted_symbols
                .iter()
                .any(|symbol| symbol.name == "middle")
        );
        assert!(
            impact
                .impacted_symbols
                .iter()
                .any(|symbol| symbol.name == "root")
        );

        let trace = index
            .trace("root", "leaf", 4)
            .expect("trace")
            .expect("root reaches leaf");
        assert_eq!(trace.path.len(), 2);
        assert_eq!(trace.path[0].caller, "root");
        assert_eq!(trace.path[0].callee, "middle");
        assert_eq!(trace.path[1].caller, "middle");
        assert_eq!(trace.path[1].callee, "leaf");
    }

    fn assert_has_call(calls: &[ExtractedCodeCall], from_symbol: &str, to_symbol: &str) {
        assert!(
            calls
                .iter()
                .any(|call| call.caller == from_symbol && call.callee == to_symbol),
            "missing {from_symbol} -> {to_symbol}"
        );
    }
}
