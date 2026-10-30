//! The environment root of a level 1 row: a saved container image, extracted once from the
//! content store into `store/rootfs/<digest>/`. Only the folders a sandbox binds are kept.
//!
//! One parser reads each archive (the `tar` crate, headers only) and the same code writes the
//! tree. The box never calls the `tar` program and never uses the crate's unpack functions.
//!
//! The outer archive is read in two passes: the first stops at `manifest.json`, the second
//! records where the data of each layer the manifest names lies, and stops once it has them
//! all. It records nothing else. Each layer is then read from where it lies. A manifest may
//! name at most `MAX_LAYERS` layers.
//!
//! One budget bounds an extraction (`WRITE_CAP` bytes). Every byte read from the archive
//! counts: in the two passes over the outer archive, and in the two passes over each layer.
//! So does every byte decoded from a gzip layer, and `ENTRY` for every entry written (folder,
//! file, symbolic link, hard link, and each parent folder a path implies) as an estimate of
//! its inode and metadata. The one count bounds the data written, the decoding work and the
//! number of entries. The outer archive is read up to twice and each layer twice more: with
//! the manifest last, as `docker save` writes it, about 8 GiB of plain layer data fits, the
//! parts outside the kept folders included. A gzip layer costs four times its compressed size
//! and twice its decoded size. An image over the budget fails and publishes nothing.
//!
//! The extraction records the path of every file and link it writes. A second count bounds
//! that memory: the bytes of every path recorded, `RECORD_CAP` in all. An image over it fails
//! and publishes nothing.
//!
//! A fence bounds the memory of the tar reader. After it yields an entry, it may read the data
//! that entry stores, padded to whole 512-byte blocks, and `EXTENSION` (1 MiB) more before it
//! yields the next one. It skips data through a fixed buffer, but holds long names, long link
//! names and PAX records in memory. The outer archive and every layer stream have a fence.
//! A GNU sparse entry is refused in both: its reported size is the expanded file, which would
//! move the fence past data the archive does not hold. A PAX-format sparse file is a regular
//! entry to the parser: its stored bytes, the sparse map included, are what the box writes.
use crate::files::temp_beside;
use crate::home::make_private_dir;
use crate::store::{Store, StoreError};
use serde::Deserialize;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Bound;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, EntryType};

/// The folders of an image that level 1 binds into the sandbox: the only ones extracted.
pub(super) const KEPT: [&str; 5] = ["usr", "bin", "lib", "lib64", "etc"];
const WHITEOUT: &[u8] = b".wh.";
const OPAQUE: &[u8] = b".wh..wh..opq";
/// Largest `manifest.json` read.
const MANIFEST_MAX: u64 = 16 << 20;
/// Most layers a manifest may name. Each costs two passes over its data.
const MAX_LAYERS: usize = 256;
/// Layer compressions by their leading bytes. Only gzip is decoded; the others are refused.
const COMPRESSIONS: [(&str, &[u8]); 4] = [
    ("gzip", &[0x1f, 0x8b]),
    ("zstd", &[0x28, 0xb5, 0x2f, 0xfd]),
    ("xz", &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]),
    ("bzip2", b"BZh"),
];
/// A tar header holds this magic at `USTAR_AT` (POSIX `ustar\0`, GNU `ustar `).
const USTAR: &[u8] = b"ustar";
const USTAR_AT: usize = 257;
/// Bytes read to tell a layer's format.
const HEAD: u64 = (USTAR_AT + USTAR.len()) as u64;
/// The extraction budget, in bytes (see the module doc).
pub const WRITE_CAP: u64 = 32 << 30;
/// What one entry written costs the budget.
const ENTRY: u64 = 4 << 10;
/// The bytes of the paths one extraction records (see the module doc).
pub const RECORD_CAP: u64 = 256 << 20;

/// What one extraction may spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The extraction budget.
    pub budget: u64,
    /// The memory of the recorded paths.
    pub records: u64,
}

/// The limits `from_store` applies.
pub const LIMITS: Limits = Limits {
    budget: WRITE_CAP,
    records: RECORD_CAP,
};
/// What the tar reader may read past an entry's stored data before it yields the next one.
const EXTENSION: u64 = 1 << 20;
/// A tar block: headers and padded data come in these.
const BLOCK: u64 = 512;

#[derive(Debug)]
pub enum RootfsError {
    /// The digest is not in the store.
    Missing(String),
    Store(StoreError),
    /// The archive or a layer is not what the image format promises.
    Format(String),
    /// A layer names a path outside its tree, or reaches a file through a link.
    Unsafe(String),
    /// The archive cannot be read.
    Tar(String),
    Io(io::Error),
}

impl fmt::Display for RootfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RootfsError::Missing(d) => write!(f, "image {d} is not in the store"),
            RootfsError::Store(e) => write!(f, "store: {e}"),
            RootfsError::Format(m) => write!(f, "bad image archive: {m}"),
            RootfsError::Unsafe(m) => write!(f, "unsafe image layer: {m}"),
            RootfsError::Tar(m) => write!(f, "cannot read the archive: {m}"),
            RootfsError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for RootfsError {}

impl From<io::Error> for RootfsError {
    fn from(e: io::Error) -> Self {
        limit(&e).map_or(RootfsError::Io(e), RootfsError::Format)
    }
}

/// A limit a read or a write crossed. It travels inside an `io::Error`, which the tar reader
/// passes on unchanged, and becomes `RootfsError::Format`.
#[derive(Debug)]
struct Limit(String);

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Limit {}

/// The message of the limit `e` carries, if any.
fn limit(e: &io::Error) -> Option<String> {
    e.get_ref()?.downcast_ref::<Limit>().map(Limit::to_string)
}

impl From<StoreError> for RootfsError {
    fn from(e: StoreError) -> Self {
        RootfsError::Store(e)
    }
}

#[derive(Deserialize)]
struct ManifestEntry {
    #[serde(rename = "Layers")]
    layers: Vec<String>,
}

/// The root of the image saved under `store_digest`, extracted on the first call and reused
/// after, even when the image is gone from the store. A crash leaves a temp folder beside the
/// result, never a partial tree at it.
pub fn from_store(store: &Store, store_digest: &str) -> Result<PathBuf, RootfsError> {
    from_store_capped(store, store_digest, LIMITS)
}

/// `from_store` with other limits. An image over one fails the call and publishes nothing.
pub fn from_store_capped(
    store: &Store,
    store_digest: &str,
    limits: Limits,
) -> Result<PathBuf, RootfsError> {
    let blob = store.path(store_digest)?;
    let parent = store.root.join("rootfs");
    make_private_dir(&parent, true, false)
        .map_err(|e| RootfsError::Io(io::Error::other(e.to_string())))?;
    let done = parent.join(store_digest);
    if is_real_dir(&done) {
        return Ok(done);
    }
    if !store.has(store_digest) {
        return Err(RootfsError::Missing(store_digest.into()));
    }
    let tmp = temp_beside(&done)?;
    fs::create_dir(&tmp)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700))?;
    let root = tmp.join("root");
    let built = fs::File::open(&blob)
        .map_err(RootfsError::from)
        .and_then(|mut f| extract(&mut f, &root, limits))
        .and_then(|()| match fs::rename(&root, &done) {
            Ok(()) => Ok(()),
            // Another box published the same tree first.
            Err(_) if is_real_dir(&done) => Ok(()),
            Err(e) => Err(e.into()),
        });
    // The temp folder is this call's own scratch; every folder in it is owner-writable.
    let _ = fs::remove_dir_all(&tmp);
    built.map(|()| done)
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

fn tar_err(e: io::Error) -> RootfsError {
    limit(&e).map_or_else(|| RootfsError::Tar(e.to_string()), RootfsError::Format)
}

/// Where the data of a file in an archive lies: offset and size, by path.
type Members = BTreeMap<PathBuf, (u64, u64)>;

fn is_file<R: Read>(entry: &tar::Entry<'_, R>) -> bool {
    matches!(
        entry.header().entry_type(),
        EntryType::Regular | EntryType::Continuous
    )
}

/// `blob` from its start, with every byte read from it charged to `budget`.
fn from_start<'a, S: Seek>(
    blob: &'a mut S,
    budget: &'a Meter,
) -> io::Result<Metered<'a, &'a mut S>> {
    blob.seek(SeekFrom::Start(0))?;
    Ok(Metered {
        inner: blob,
        meter: budget,
    })
}

/// The bytes of the first `manifest.json` in the archive, from a pass that stops there.
fn read_manifest(blob: &mut (impl Read + Seek), budget: &Meter) -> Result<Vec<u8>, RootfsError> {
    let name = Path::new("manifest.json");
    each_entry(from_start(blob, budget)?, |entry| {
        if !is_file(entry) || normalize(&entry.path_bytes())?.as_deref() != Some(name) {
            return Ok(None);
        }
        let mut text = Vec::new();
        entry.take(MANIFEST_MAX).read_to_end(&mut text)?;
        Ok(Some(text))
    })?
    .ok_or_else(|| RootfsError::Format("manifest.json is missing".into()))
}

/// Where the data of each file in `names` lies, from a pass that stops once it has found
/// them all. The first file of a name wins; no other file is recorded.
fn find_layers(
    blob: &mut (impl Read + Seek),
    names: &[PathBuf],
    budget: &Meter,
) -> Result<Members, RootfsError> {
    let wanted: BTreeSet<&Path> = names.iter().map(PathBuf::as_path).collect();
    let mut found = Members::new();
    each_entry(from_start(blob, budget)?, |entry| {
        if is_file(entry)
            && let Some(path) = normalize(&entry.path_bytes())?
            && wanted.contains(path.as_path())
        {
            let at = (entry.raw_file_position(), entry.size());
            found.entry(path).or_insert(at);
        }
        Ok((found.len() == wanted.len()).then_some(()))
    })?;
    Ok(found)
}

/// Call `each` on the entries of the tar in `reader`, behind a fence, until it returns
/// `Some`; that value comes back.
fn each_entry<R: Read, T>(
    reader: R,
    mut each: impl FnMut(&mut tar::Entry<'_, Metered<'_, R>>) -> Result<Option<T>, RootfsError>,
) -> Result<Option<T>, RootfsError> {
    let fence = Meter::fence();
    let mut ar = Archive::new(Metered {
        inner: reader,
        meter: &fence,
    });
    for entry in ar.entries().map_err(tar_err)? {
        let mut entry = entry.map_err(tar_err)?;
        fence.past(&entry)?;
        if let Some(found) = each(&mut entry)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Extract the image archive `blob` into `root`. One pass over the archive finds the
/// manifest, a second finds the layers it names, and each layer is then read from where it
/// lies. The budget counts every byte the two passes read.
fn extract(blob: &mut (impl Read + Seek), root: &Path, limits: Limits) -> Result<(), RootfsError> {
    let budget = Meter::budget(limits.budget);
    let manifest = read_manifest(blob, &budget)?;
    let manifest: Vec<ManifestEntry> = serde_json::from_slice(&manifest)
        .map_err(|e| RootfsError::Format(format!("manifest.json: {e}")))?;
    let layers = manifest
        .into_iter()
        .next()
        .map(|m| m.layers)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| RootfsError::Format("manifest.json lists no layers".into()))?;
    if layers.len() > MAX_LAYERS {
        return Err(RootfsError::Format(format!(
            "manifest.json lists {} layers, over the limit of {MAX_LAYERS}",
            layers.len()
        )));
    }
    let mut names = Vec::new();
    for l in &layers {
        match normalize(l.as_bytes()) {
            Ok(Some(p)) if p == Path::new(l) => names.push(p),
            _ => return Err(RootfsError::Unsafe(format!("layer path {l:?}"))),
        }
    }
    let found = find_layers(blob, &names, &budget)?;
    fs::create_dir(root)?;
    let records = Meter::records(limits.records);
    let mut tree = Tree::new(root, &budget, &records);
    for name in &names {
        let &(at, size) = found
            .get(name)
            .ok_or_else(|| RootfsError::Format(format!("layer {} is missing", name.display())))?;
        for pass in [Pass::Whiteouts, Pass::Content] {
            blob.seek(SeekFrom::Start(at))?;
            tree.layer(&mut Read::take(&mut *blob, size), pass)?;
        }
    }
    Ok(())
}

/// A count of bytes against an end. A charge past the end fails with the `over` message.
struct Meter {
    used: Cell<u64>,
    end: Cell<u64>,
    over: String,
}

impl Meter {
    fn new(end: u64, over: String) -> Meter {
        Meter {
            used: Cell::new(0),
            end: Cell::new(end),
            over,
        }
    }

    /// The extraction budget: `cap` bytes in all.
    fn budget(cap: u64) -> Meter {
        Meter::new(
            cap,
            format!(
                "the image is over the extraction budget of {cap} bytes (archive bytes read \
                 to find its members and in two passes over each layer, bytes decoded from \
                 gzip layers, and {ENTRY} per entry written)"
            ),
        )
    }

    /// The paths an extraction records: `cap` bytes in all.
    fn records(cap: u64) -> Meter {
        Meter::new(
            cap,
            format!("the paths the extraction records are over {cap} bytes"),
        )
    }

    /// The fence of one tar stream, counted from its start: `EXTENSION` bytes until the reader
    /// yields the first entry.
    fn fence() -> Meter {
        Meter::new(
            EXTENSION,
            format!("a header extension is larger than {} MiB", EXTENSION >> 20),
        )
    }

    /// Count `n`; an error once the count is past the end.
    fn charge(&self, n: u64) -> io::Result<()> {
        self.used.set(self.used.get().saturating_add(n));
        if self.used.get() > self.end.get() {
            return Err(io::Error::other(Limit(self.over.clone())));
        }
        Ok(())
    }

    /// Bytes the next read may take: up to the end, and one more to show it was crossed.
    fn room(&self) -> u64 {
        self.end
            .get()
            .saturating_sub(self.used.get())
            .saturating_add(1)
    }

    /// For a fence: the reader yielded `entry`, so it may read the data the entry stores, padded
    /// to whole blocks, then `EXTENSION` more for the next header and its extensions.
    ///
    /// A GNU sparse entry is refused: the reader reports its expanded size, not the data it
    /// stores. For every other entry the size is the stored data, the header's size or the PAX
    /// `size` record that replaces it, which is what the reader skips.
    fn past<R: Read>(&self, entry: &tar::Entry<'_, R>) -> Result<(), RootfsError> {
        if entry.header().entry_type().is_gnu_sparse() {
            return Err(RootfsError::Format(format!(
                "{} is a GNU sparse file (image archives do not use them)",
                String::from_utf8_lossy(&entry.path_bytes())
            )));
        }
        let stored = entry.size().div_ceil(BLOCK).saturating_mul(BLOCK);
        let data_end = entry.raw_file_position().saturating_add(stored);
        self.end.set(data_end.saturating_add(EXTENSION));
        Ok(())
    }
}

/// A stream that charges every byte it yields to `meter`, and never reads more than one byte
/// past the meter's end.
struct Metered<'a, R> {
    inner: R,
    meter: &'a Meter,
}

impl<R: Read> Read for Metered<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let room = usize::try_from(self.meter.room()).unwrap_or(usize::MAX);
        let take = buf.len().min(room);
        let n = self.inner.read(&mut buf[..take])?;
        self.meter.charge(n as u64)?;
        Ok(n)
    }
}

/// The compression of a layer that starts with `head`, or `None` for a plain tar: one with the
/// ustar magic, or one no compression claims (an old tar has no magic).
fn compression(head: &[u8]) -> Option<&'static str> {
    if head.get(USTAR_AT..).is_some_and(|m| m.starts_with(USTAR)) {
        return None;
    }
    COMPRESSIONS
        .iter()
        .find(|(_, magic)| head.starts_with(magic))
        .map(|(name, _)| *name)
}

/// A relative path of normal components. A leading `./` and empty parts drop out; `None`
/// when nothing is left. A root or `..` is an error.
fn normalize(bytes: &[u8]) -> Result<Option<PathBuf>, RootfsError> {
    let path = Path::new(OsStr::from_bytes(bytes));
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(p) => out.push(p),
            _ => return Err(RootfsError::Unsafe(format!("path {path:?}"))),
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// True when the path's first component is a kept folder.
fn in_kept(p: &Path) -> bool {
    first(p).is_some_and(|f| KEPT.iter().any(|k| f.as_bytes() == k.as_bytes()))
}

fn first(p: &Path) -> Option<&OsStr> {
    p.components().next().map(Component::as_os_str)
}

#[derive(Clone, Copy)]
enum Pass {
    Whiteouts,
    Content,
}

/// The tree under construction, and an exact record of what this extraction made: every
/// symbolic link and every regular file, by path relative to the root. `records` counts the
/// bytes of every path recorded.
struct Tree<'a> {
    root: &'a Path,
    links: BTreeSet<PathBuf>,
    files: BTreeSet<PathBuf>,
    budget: &'a Meter,
    records: &'a Meter,
}

impl<'a> Tree<'a> {
    fn new(root: &'a Path, budget: &'a Meter, records: &'a Meter) -> Tree<'a> {
        Tree {
            root,
            links: BTreeSet::new(),
            files: BTreeSet::new(),
            budget,
            records,
        }
    }

    /// `rel`, to record, once its bytes are counted.
    fn kept(&self, rel: &Path) -> io::Result<PathBuf> {
        self.records.charge(rel.as_os_str().len() as u64)?;
        Ok(rel.to_path_buf())
    }

    /// One pass over the layer's members.
    fn layer(&mut self, layer: &mut dyn Read, pass: Pass) -> Result<(), RootfsError> {
        // Every byte read from the layer counts, and for gzip every decoded byte as well: a
        // decoder can read many bytes that decode to nothing.
        let mut raw = Metered {
            inner: layer,
            meter: self.budget,
        };
        let mut head = Vec::new();
        (&mut raw).take(HEAD).read_to_end(&mut head)?;
        let kind = compression(&head);
        let joined = io::Cursor::new(head).chain(raw);
        // Decoded here, so the same reader parses a plain and a gzip layer.
        let stream: Box<dyn Read + '_> = match kind {
            None => Box::new(joined),
            Some("gzip") => Box::new(Metered {
                inner: flate2::read::MultiGzDecoder::new(joined),
                meter: self.budget,
            }),
            Some(name) => {
                return Err(RootfsError::Format(format!(
                    "unsupported compression: {name}"
                )));
            }
        };
        each_entry(stream, |entry| {
            if let Some(path) = normalize(&entry.path_bytes())? {
                match pass {
                    Pass::Whiteouts => self.whiteout(&path)?,
                    Pass::Content => self.member(entry, &path)?,
                }
            }
            Ok(None::<()>)
        })?;
        Ok(())
    }

    /// Apply the whiteout `path` names, if it is one: `.wh.<name>` removes `<name>` beside it;
    /// `.wh..wh..opq` empties its folder; at the root, `.wh.<kept>` removes that folder. A
    /// `<name>` of `.` or `..` names no entry beside the whiteout, so it is no whiteout.
    fn whiteout(&mut self, path: &Path) -> Result<(), RootfsError> {
        let Some(base) = path.file_name().map(OsStrExt::as_bytes) else {
            return Ok(());
        };
        let Some(named) = base
            .strip_prefix(WHITEOUT)
            .filter(|n| !matches!(*n, b"" | b"." | b".."))
        else {
            return Ok(());
        };
        let parent = path.parent().unwrap_or(Path::new(""));
        if parent.as_os_str().is_empty() {
            let target = Path::new(OsStr::from_bytes(named));
            // The root stays; only a whole kept folder can go.
            if base != OPAQUE && in_kept(target) && target.components().count() == 1 {
                self.remove(target)?;
            }
        } else if in_kept(path) {
            if base == OPAQUE {
                self.empty(parent)?;
            } else {
                self.remove(&parent.join(OsStr::from_bytes(named)))?;
            }
        }
        Ok(())
    }

    fn member(
        &mut self,
        entry: &mut tar::Entry<'_, impl Read>,
        path: &Path,
    ) -> Result<(), RootfsError> {
        let base = path.file_name().map(OsStrExt::as_bytes).unwrap_or_default();
        if !in_kept(path) || base.starts_with(WHITEOUT) {
            return Ok(());
        }
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        match entry.header().entry_type() {
            EntryType::Directory => self.dir(path, mode)?,
            EntryType::Regular | EntryType::Continuous => self.file(entry, path, mode)?,
            kind @ (EntryType::Symlink | EntryType::Link) => {
                let target = entry.link_name_bytes().ok_or_else(|| {
                    RootfsError::Format(format!("link {} has no target", path.display()))
                })?;
                if kind == EntryType::Symlink {
                    self.symlink(path, &target)?
                } else {
                    self.hard_link(path, &target)?
                }
            }
            // Devices, pipes, sockets and the rest are not part of the root.
            _ => return Ok(()),
        }
        Ok(self.budget.charge(ENTRY)?)
    }

    fn full(&self, rel: &Path) -> PathBuf {
        self.root.join(rel)
    }

    /// Refuse a path with a link among its parent folders, in the record or on disk.
    /// With `create`, make the missing parent folders.
    fn parents(&self, rel: &Path, create: bool) -> Result<(), RootfsError> {
        let mut at = PathBuf::new();
        let depth = rel.components().count();
        for c in rel.components().take(depth.saturating_sub(1)) {
            at.push(c);
            let through = || {
                RootfsError::Unsafe(format!(
                    "{} is reached through the link {}",
                    rel.display(),
                    at.display()
                ))
            };
            if self.links.contains(&at) {
                return Err(through());
            }
            match fs::symlink_metadata(self.full(&at)) {
                Ok(m) if m.is_dir() => {}
                Ok(m) if m.is_symlink() => return Err(through()),
                Ok(_) => {
                    return Err(RootfsError::Format(format!(
                        "{} is a file, not a folder",
                        at.display()
                    )));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
                    fs::create_dir(self.full(&at))?;
                    self.budget.charge(ENTRY)?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Forget every record at or under `rel`. Paths order by component, so `rel` and its
    /// descendants form one range that starts at `rel`.
    fn forget(&mut self, rel: &Path) {
        for set in [&mut self.links, &mut self.files] {
            let under: Vec<PathBuf> = set
                .range::<Path, _>((Bound::Included(rel), Bound::Unbounded))
                .take_while(|p| p.starts_with(rel))
                .cloned()
                .collect();
            for p in &under {
                set.remove(p);
            }
        }
    }

    /// Remove what is at `rel` without following a link; absent is fine.
    fn clear(&mut self, rel: &Path) -> Result<(), RootfsError> {
        let full = self.full(rel);
        match fs::symlink_metadata(&full) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(&full)?,
            Ok(_) => fs::remove_file(&full)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        self.forget(rel);
        Ok(())
    }

    /// A whiteout's removal: the parents may not be links.
    fn remove(&mut self, rel: &Path) -> Result<(), RootfsError> {
        self.parents(rel, false)?;
        self.clear(rel)
    }

    /// Remove everything inside the folder `dir`, keeping the folder.
    fn empty(&mut self, dir: &Path) -> Result<(), RootfsError> {
        self.parents(dir, false)?;
        let full = self.full(dir);
        match fs::symlink_metadata(&full) {
            Ok(m) if m.is_dir() => {}
            Ok(m) if m.is_symlink() => {
                return Err(RootfsError::Unsafe(format!("{} is a link", dir.display())));
            }
            _ => return Ok(()),
        }
        for e in fs::read_dir(&full)? {
            self.clear(&dir.join(e?.file_name()))?;
        }
        Ok(())
    }

    fn dir(&mut self, rel: &Path, mode: u32) -> Result<(), RootfsError> {
        self.parents(rel, true)?;
        let full = self.full(rel);
        if !is_real_dir(&full) {
            self.clear(rel)?;
            fs::create_dir(&full)?;
        }
        // Owner-writable, so a later layer can fill the folder.
        fs::set_permissions(&full, fs::Permissions::from_mode(mode | 0o700))?;
        Ok(())
    }

    /// A new file: whatever is there goes first, then the file is created exclusively without
    /// following a link, with the permission bits only (no setuid, setgid or sticky).
    fn file(&mut self, data: &mut impl Read, rel: &Path, mode: u32) -> Result<(), RootfsError> {
        self.parents(rel, true)?;
        self.clear(rel)?;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(mode)
            .open(self.full(rel))?;
        io::copy(data, &mut f)?;
        self.files.insert(self.kept(rel)?);
        Ok(())
    }

    fn symlink(&mut self, rel: &Path, target: &[u8]) -> Result<(), RootfsError> {
        self.parents(rel, true)?;
        self.clear(rel)?;
        std::os::unix::fs::symlink(OsStr::from_bytes(target), self.full(rel))?;
        self.links.insert(self.kept(rel)?);
        Ok(())
    }

    /// A hard link to a regular file this extraction wrote in a kept folder, reached through
    /// no link. The link keeps the target's content as it is now.
    fn hard_link(&mut self, rel: &Path, target: &[u8]) -> Result<(), RootfsError> {
        let unsafe_ = |why: &str| {
            RootfsError::Unsafe(format!(
                "{} is a hard link to {:?}: {why}",
                rel.display(),
                OsStr::from_bytes(target)
            ))
        };
        let Some(to) = normalize(target)? else {
            return Err(unsafe_("no target"));
        };
        if !in_kept(&to) {
            return Err(unsafe_("outside the kept folders"));
        }
        let mut at = PathBuf::new();
        for c in to.components() {
            at.push(c);
            if self.links.contains(&at) {
                return Err(unsafe_("through a link"));
            }
        }
        let on_disk = fs::symlink_metadata(self.full(&to)).is_ok_and(|m| m.is_file());
        if !self.files.contains(&to) || !on_disk {
            return Err(unsafe_("not a file this extraction wrote"));
        }
        self.parents(&to, false)?;
        self.parents(rel, true)?;
        self.clear(rel)?;
        fs::hard_link(self.full(&to), self.full(rel))?;
        self.files.insert(self.kept(rel)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tar of regular files.
    fn tar_of(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, data) in members {
            let mut h = tar::Header::new_gnu();
            h.set_path(name).unwrap();
            h.set_mode(0o644);
            h.set_size(data.len() as u64);
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    /// A source that counts the bytes read through it.
    struct Counted<R> {
        inner: R,
        read: u64,
    }

    impl<R: Read> Read for Counted<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read += n as u64;
            Ok(n)
        }
    }

    impl<R: Seek> Seek for Counted<R> {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.inner.seek(to)
        }
    }

    #[test]
    fn a_manifest_after_a_large_member_costs_two_passes_however_many_layers_it_names() {
        let layers = [tar_of(&[("usr/a", b"A")]), tar_of(&[("usr/b", b"B")])];
        let filler = vec![0u8; 8 << 20];
        let manifest = br#"[{"Layers":["l0/layer.tar","l1/layer.tar","l0/layer.tar"]}]"#;
        // `docker save` writes the manifest last.
        let image = tar_of(&[
            ("filler", &filler),
            ("l0/layer.tar", &layers[0]),
            ("l1/layer.tar", &layers[1]),
            ("manifest.json", manifest),
        ]);
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("root");
        let mut blob = Counted {
            inner: io::Cursor::new(&image),
            read: 0,
        };
        extract(&mut blob, &root, LIMITS).unwrap();
        assert_eq!(fs::read_to_string(root.join("usr/b")).unwrap(), "B");
        // Two passes over the archive, then two passes over each named layer from where it
        // lies.
        let named = 2 * layers[0].len() + layers[1].len();
        let most = (2 * image.len() + 2 * named) as u64;
        assert!(
            (filler.len() as u64..=most).contains(&blob.read),
            "{} bytes read; two passes are at most {most}",
            blob.read
        );
    }

    #[test]
    fn the_layer_pass_records_only_the_named_files_and_stops_at_the_last() {
        let fillers: Vec<String> = (0..1000).map(|i| format!("f{i}")).collect();
        let mut members: Vec<(&str, &[u8])> =
            fillers.iter().map(|f| (f.as_str(), &b""[..])).collect();
        members.extend([("l0/layer.tar", &b"x"[..]), ("after", &b"y"[..])]);
        let image = tar_of(&members);
        let budget = Meter::budget(WRITE_CAP);
        let names = [PathBuf::from("l0/layer.tar"), PathBuf::from("l0/layer.tar")];
        let found = find_layers(&mut io::Cursor::new(&image), &names, &budget).unwrap();
        // 1000 empty fillers of one header each, then the layer's header.
        let at = 512 * 1001;
        assert_eq!(found, Members::from([(names[0].clone(), (at, 1))]));
        assert_eq!(budget.used.get(), at);
        // A name the archive lacks: the pass reads on, past the layer's data block, the last
        // file's header and block, to the end block, and finds nothing.
        let missing = [PathBuf::from("l9/layer.tar")];
        let budget = Meter::budget(WRITE_CAP);
        let found = find_layers(&mut io::Cursor::new(&image), &missing, &budget).unwrap();
        assert!(found.is_empty());
        assert_eq!(budget.used.get(), at + 4 * 512);
    }

    #[test]
    fn forget_drops_a_path_and_its_descendants_and_keeps_its_neighbours() {
        let budget = Meter::budget(WRITE_CAP);
        let records = Meter::records(RECORD_CAP);
        let mut tree = Tree::new(Path::new("/nonexistent"), &budget, &records);
        let all = [
            "usr", "usr/a", "usr/a/b", "usr/a-b", "usr/a.b", "usr/a0", "usr/ab",
        ];
        for p in all {
            tree.links.insert(p.into());
            tree.files.insert(p.into());
        }
        tree.forget(Path::new("usr/a"));
        let kept = ["usr", "usr/a-b", "usr/a.b", "usr/a0", "usr/ab"].map(PathBuf::from);
        assert!(tree.links.iter().eq(&kept), "{:?}", tree.links);
        assert!(tree.files.iter().eq(&kept), "{:?}", tree.files);
        // Only the descendants left: they go too. An absent path takes nothing.
        tree.links.insert("usr/a0/x".into());
        tree.forget(Path::new("usr/a0"));
        tree.forget(Path::new("usr/b"));
        let kept = ["usr", "usr/a-b", "usr/a.b", "usr/ab"].map(PathBuf::from);
        assert!(tree.links.iter().eq(&kept), "{:?}", tree.links);
    }
}
