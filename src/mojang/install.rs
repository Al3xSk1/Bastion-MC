//! Verified download store.
//!
//! Rules enforced here:
//!  * A file reaches its final path only after its digest and size matched (temp file with
//!    `O_EXCL`, hash while writing, `fsync`, atomic rename).
//!  * A digest mismatch on downloaded data is never retried and never "repaired": it aborts.
//!  * A file already in the store that no longer matches is treated as tampering/corruption
//!    and aborts too, unless the user explicitly asked for `--repair`.
//!  * At launch every file is hashed again *from the file descriptor that is then handed to
//!    the sandbox*, so nothing can be swapped between verification and use.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use hyper::Method;

use super::meta::{Artifact, AssetIndex, NativeArtifact, VERSION_MANIFEST_URL, VersionJson, VersionManifest};
use crate::{
    error::{Context, Error, Result},
    events::{Progress, Reporter, Unit},
    net::http::{Connection, HttpClient, Sink},
    paths::{Paths, validate_file_name, validate_relative},
    util::{self, Expected, Hasher},
};

const MANIFEST_LIMIT: u64 = 16 << 20;
const VERSION_JSON_LIMIT: u64 = 4 << 20;
const UNKNOWN_SIZE_LIMIT: u64 = 512 << 20;
const NATIVE_FILE_LIMIT: u64 = 128 << 20;
const NATIVE_TOTAL_LIMIT: u64 = 512 << 20;

/// An artifact together with the store root it lives under.
#[derive(Debug, Clone)]
pub struct Stored {
    pub artifact: Artifact,
    pub path: PathBuf,
}

impl Stored {
    fn new(root: &Path, artifact: Artifact) -> Result<Self> {
        validate_relative(Path::new(&artifact.rel_path))?;
        Ok(Stored { path: root.join(&artifact.rel_path), artifact })
    }

    /// Open without following symlinks, freeze the file with fs-verity where the filesystem
    /// allows, then verify digest + size on the open descriptor. Freezing first means the
    /// bytes cannot be rewritten in place after they have been hashed.
    pub fn open_verified(&self) -> Result<File> {
        let file = util::open_regular_nofollow(&self.path)
            .ctx(|| format!("opening {} ({})", self.path.display(), self.artifact.name))?;
        let _ = crate::verity::enable(&file);
        self.artifact.expect.verify_file(&file, self.artifact.size, &self.artifact.name)?;
        Ok(file)
    }
}

/// Everything a game version needs, present and verified in the local store.
#[derive(Debug, Clone)]
pub struct Installed {
    pub json: VersionJson,
    pub json_sha1: String,
    pub client: Stored,
    pub libraries: Vec<Stored>,
    pub natives: Vec<(Stored, Vec<String>)>,
    pub asset_index: Stored,
    pub assets: AssetIndex,
    pub asset_objects: Vec<Stored>,
    pub logging: Option<(String, Stored)>,
    /// `assets/virtual/<index>` for versions that need assets under their logical names.
    pub logical_assets: Option<PathBuf>,
}

/// Files re-verified for one launch; each `File` is the descriptor the sandbox will see.
pub struct LaunchFiles {
    pub client: File,
    pub libraries: Vec<(Stored, File)>,
    pub natives: Vec<(Stored, Vec<String>, File)>,
    pub asset_index: File,
    pub logging: Option<(String, Stored, File)>,
}

pub struct Store<'a> {
    pub paths: &'a Paths,
    pub http: &'a HttpClient,
    pub parallel: usize,
    /// Re-download files whose stored copy no longer matches instead of aborting.
    pub repair: bool,
    /// Where status notes and download progress go.
    pub reporter: &'a Reporter,
}

impl Store<'_> {
    fn note(&self, msg: &str) {
        self.reporter.status(msg);
    }

    pub async fn fetch_manifest(&self) -> Result<VersionManifest> {
        let bytes = self.http.get(VERSION_MANIFEST_URL, MANIFEST_LIMIT).await?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Format(format!("version manifest: {e}")))
    }

    /// Make a version fully present. `pinned_json_sha1` (from a signed profile) must equal
    /// what Mojang currently publishes, otherwise the launch is refused.
    pub async fn install(&self, version_id: &str, pinned_json_sha1: Option<&Expected>) -> Result<Installed> {
        crate::config::validate_version_id(version_id)?;
        self.note(&format!("resolving Minecraft {version_id} from Mojang's version manifest"));
        let manifest = self.fetch_manifest().await?;
        let entry = manifest.find(version_id)?;
        let json_expect = Expected::sha1(&entry.sha1)?;
        if let Some(pin) = pinned_json_sha1
            && pin != &json_expect
        {
            return Err(Error::Integrity(format!(
                "version file for {version_id}: Mojang publishes sha1 {}, but the signed profile pins {}; \
                 re-review and re-sign the profile if Mojang re-issued this version",
                json_expect.hex(),
                pin.hex()
            )));
        }

        let json_artifact = Artifact {
            name: format!("version file {version_id}"),
            rel_path: format!("{0}/{0}.json", version_id),
            url: entry.url.clone(),
            expect: json_expect.clone(),
            size: None,
        };
        let json_stored = Stored::new(&self.paths.versions(), json_artifact)?;
        self.ensure(vec![json_stored.clone()], VERSION_JSON_LIMIT, "version file").await?;
        let json_bytes = read_verified(&json_stored)?;
        let json = VersionJson::parse(&json_bytes)?;
        if json.id != version_id {
            return Err(Error::Integrity(format!("version file claims id {:?}, expected {version_id:?}", json.id)));
        }

        let libs_root = self.paths.libraries();
        let assets_root = self.paths.assets();
        let client = Stored::new(&self.paths.versions(), json.client_artifact()?)?;
        let libraries =
            json.classpath_libraries()?.into_iter().map(|a| Stored::new(&libs_root, a)).collect::<Result<Vec<_>>>()?;
        let natives = json
            .native_archives()?
            .into_iter()
            .map(|NativeArtifact { artifact, exclude }| Ok((Stored::new(&libs_root, artifact)?, exclude)))
            .collect::<Result<Vec<_>>>()?;
        let asset_index = Stored::new(&assets_root, json.asset_index_artifact()?)?;
        let logging = match json.logging_artifact()? {
            Some((arg, art)) => Some((arg, Stored::new(&assets_root, art)?)),
            None => None,
        };

        let mut jobs = vec![client.clone(), asset_index.clone()];
        jobs.extend(libraries.iter().cloned());
        jobs.extend(natives.iter().map(|(s, _)| s.clone()));
        jobs.extend(logging.iter().map(|(_, s)| s.clone()));
        self.ensure(jobs, UNKNOWN_SIZE_LIMIT, "game files").await?;

        let assets = AssetIndex::parse(&read_verified(&asset_index)?)?;
        let asset_objects =
            assets.unique_objects()?.into_iter().map(|a| Stored::new(&assets_root, a)).collect::<Result<Vec<_>>>()?;
        self.ensure(asset_objects.clone(), UNKNOWN_SIZE_LIMIT, "assets").await?;

        let logical_assets = if assets.needs_logical_tree() {
            let root = assets_root.join("virtual").join(&json.asset_index.id);
            build_logical_tree(&assets, &assets_root, &root)?;
            Some(root)
        } else {
            None
        };

        Ok(Installed {
            json_sha1: json_expect.hex().to_string(),
            json,
            client,
            libraries,
            natives,
            asset_index,
            assets,
            asset_objects,
            logging,
            logical_assets,
        })
    }

    /// Bring every artifact into the store: verify what is there, download what is not.
    pub async fn ensure(&self, items: Vec<Stored>, default_limit: u64, label: &str) -> Result<()> {
        let total = items.len();
        let threads = self.parallel;
        let checked = tokio::task::spawn_blocking(move || classify(items, threads))
            .await
            .map_err(|e| Error::Other(format!("verification task failed: {e}")))??;

        if !checked.corrupt.is_empty() {
            if !self.repair {
                let first = &checked.corrupt[0];
                return Err(Error::Integrity(format!(
                    "{} stored file(s) no longer match their published digest, first: {} ({}). \
                     This is corruption or tampering. Inspect it, then re-run with --repair to re-download.",
                    checked.corrupt.len(),
                    first.0.path.display(),
                    first.1
                )));
            }
            for (stored, _) in &checked.corrupt {
                self.note(&format!("repair: discarding mismatching {}", stored.path.display()));
                fs::remove_file(&stored.path).ctx(|| format!("removing {}", stored.path.display()))?;
            }
        }
        let mut missing = checked.missing;
        missing.extend(checked.corrupt.into_iter().map(|(s, _)| s));
        if missing.is_empty() {
            return Ok(());
        }
        self.note(&format!("{label}: {} of {total} to download", missing.len()));
        // Measured in bytes when every size is published (the usual case), in files otherwise.
        let known_bytes: Option<u64> = missing.iter().map(|s| s.artifact.size).sum();
        let progress = match known_bytes {
            Some(bytes) if bytes > 0 => self.reporter.progress(label, bytes, Unit::Bytes),
            _ => self.reporter.progress(label, missing.len() as u64, Unit::Files),
        };

        let mut dirs: Vec<&Path> = missing.iter().filter_map(|s| s.path.parent()).collect();
        dirs.sort_unstable();
        dirs.dedup();
        for dir in dirs {
            sweep_stale_parts(dir);
        }

        // Group by host so each worker can keep one TLS connection alive.
        let mut by_host: BTreeMap<String, VecDeque<(Stored, String)>> = BTreeMap::new();
        for stored in missing {
            let target = self.http.parse_url(&stored.artifact.url)?;
            by_host.entry(target.host).or_default().push_back((stored, target.path));
        }
        let mut tasks = tokio::task::JoinSet::new();
        for (host, queue) in by_host {
            let workers = self.parallel.min(queue.len()).max(1);
            let queue = Arc::new(Mutex::new(queue));
            for _ in 0..workers {
                let (http, host, queue, progress) = (self.http.clone(), host.clone(), queue.clone(), progress.clone());
                tasks.spawn(async move { download_worker(http, host, queue, default_limit, progress).await });
            }
        }
        let mut first_err = None;
        while let Some(joined) = tasks.join_next().await {
            let result = joined.unwrap_or_else(|e| Err(Error::Other(format!("download task failed: {e}"))));
            if let Err(e) = result
                && first_err.is_none()
            {
                // Stop the remaining workers: after an integrity failure nothing else matters.
                tasks.abort_all();
                first_err = Some(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => {
                progress.complete();
                Ok(())
            }
        }
    }
}

impl Installed {
    /// Re-verify everything that will be on the class path or parsed by the game, returning
    /// the open descriptors. Asset objects are re-hashed too unless disabled.
    pub fn verify_for_launch(&self, verify_assets: bool, threads: usize) -> Result<LaunchFiles> {
        let client = self.client.open_verified()?;
        let asset_index = self.asset_index.open_verified()?;
        let lib_files = verify_parallel(&self.libraries, threads)?;
        let native_stored: Vec<Stored> = self.natives.iter().map(|(s, _)| s.clone()).collect();
        let native_files = verify_parallel(&native_stored, threads)?;
        let logging = match &self.logging {
            Some((arg, stored)) => Some((arg.clone(), stored.clone(), stored.open_verified()?)),
            None => None,
        };
        if verify_assets {
            verify_parallel(&self.asset_objects, threads)?;
            if let Some(root) = &self.logical_assets {
                verify_logical_tree(&self.assets, root)?;
            }
        }
        Ok(LaunchFiles {
            client,
            libraries: self.libraries.iter().cloned().zip(lib_files).collect(),
            natives: self
                .natives
                .iter()
                .cloned()
                .zip(native_files)
                .map(|((stored, exclude), file)| (stored, exclude, file))
                .collect(),
            asset_index,
            logging,
        })
    }
}

fn read_verified(stored: &Stored) -> Result<Vec<u8>> {
    let mut file = stored.open_verified()?;
    let mut buf = Vec::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_end(&mut buf))
        .ctx(|| format!("reading {}", stored.path.display()))?;
    // The bytes just read must be the bytes that were verified.
    if !stored.artifact.expect.matches(&util::hash_bytes(stored.artifact.expect.alg, &buf)) {
        return Err(Error::Integrity(format!("{} changed while being read", stored.path.display())));
    }
    Ok(buf)
}

struct Classified {
    missing: Vec<Stored>,
    corrupt: Vec<(Stored, String)>,
}

/// Run `f` over `items` on `threads` worker threads, preserving order.
fn parallel_map<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, 32).min(items.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    *slots[i].lock().expect("slot lock") = Some(f(item));
                }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().expect("slot lock").expect("slot filled")).collect()
}

fn classify(items: Vec<Stored>, threads: usize) -> Result<Classified> {
    enum State {
        Ok,
        Missing,
        Corrupt(String),
    }
    let states = parallel_map(&items, threads, |stored| -> Result<State> {
        match fs::symlink_metadata(&stored.path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(State::Missing),
            Err(e) => Err(Error::io(format!("inspecting {}", stored.path.display()), e)),
            Ok(_) => match stored.open_verified() {
                Ok(_) => Ok(State::Ok),
                Err(Error::Integrity(why)) => Ok(State::Corrupt(why)),
                // A symlink or special file where a jar should be is not "missing".
                Err(Error::Io { context, source }) => Ok(State::Corrupt(format!("{context}: {source}"))),
                Err(e) => Err(e),
            },
        }
    });
    let mut out = Classified { missing: Vec::new(), corrupt: Vec::new() };
    for (stored, state) in items.into_iter().zip(states) {
        match state? {
            State::Ok => {}
            State::Missing => out.missing.push(stored),
            State::Corrupt(why) => out.corrupt.push((stored, why)),
        }
    }
    Ok(out)
}

/// Verify a batch and return the open descriptors in the same order.
fn verify_parallel(items: &[Stored], threads: usize) -> Result<Vec<File>> {
    parallel_map(items, threads, |s| s.open_verified()).into_iter().collect()
}

// ---------------------------------------------------------------- downloading

struct FileSink<'a> {
    file: File,
    hasher: Hasher,
    written: u64,
    /// Byte-level progress (when the batch is measured in bytes) and what this attempt has
    /// contributed to it so far, so a failed attempt can be taken back.
    progress: Option<(&'a Progress, &'a AtomicU64)>,
}

impl Sink for FileSink<'_> {
    fn accept(&mut self, chunk: &[u8]) -> Result<()> {
        self.hasher.update(chunk);
        self.file.write_all(chunk).ctx(|| "writing download")?;
        self.written += chunk.len() as u64;
        if let Some((progress, counted)) = self.progress {
            counted.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            progress.add(chunk.len() as u64);
        }
        Ok(())
    }
}

async fn download_worker(
    http: HttpClient,
    host: String,
    queue: Arc<Mutex<VecDeque<(Stored, String)>>>,
    default_limit: u64,
    progress: Arc<Progress>,
) -> Result<()> {
    let mut conn: Option<Connection> = None;
    loop {
        let Some((stored, path)) = queue.lock().expect("queue lock").pop_front() else { return Ok(()) };
        let mut attempt = 0;
        loop {
            let c = match conn.as_mut() {
                Some(c) => c,
                None => conn.insert(http.connect(&host).await?),
            };
            match download_one(c, &stored, &path, default_limit, &progress).await {
                Ok(()) => break,
                // Wrong bytes from an authenticated server: stop. Retrying would only give an
                // attacker more attempts.
                Err(e @ (Error::Integrity(_) | Error::Policy(_))) => return Err(e),
                Err(e) => {
                    conn = None;
                    attempt += 1;
                    if attempt >= 3 {
                        return Err(e);
                    }
                }
            }
        }
        if progress.unit() == Unit::Files {
            progress.add(1);
        }
    }
}

/// A download in progress. Unless [`keep`](Self::keep) is called the file is removed when
/// this goes out of scope — on an error, and equally when the download is cancelled and its
/// future is simply dropped.
struct PartFile {
    path: PathBuf,
    keep: bool,
}

impl PartFile {
    const PREFIX: &'static str = ".part-";

    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Remove download leftovers that no destructor got to run for (the process was killed).
/// Only files old enough that no running download can own them are touched.
fn sweep_stale_parts(dir: &Path) {
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let is_part = entry.file_name().to_string_lossy().starts_with(PartFile::PREFIX);
        let Ok(meta) = entry.metadata() else { continue };
        let stale = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > STALE_AFTER);
        if is_part && meta.file_type().is_file() && stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

async fn download_one(
    conn: &mut Connection,
    stored: &Stored,
    path: &str,
    default_limit: u64,
    progress: &Progress,
) -> Result<()> {
    let art = &stored.artifact;
    let dir = stored.path.parent().ok_or_else(|| Error::Other("artifact path has no parent".into()))?;
    util::create_private_dir_all(dir)?;
    let tmp = dir.join(format!("{}{}", PartFile::PREFIX, util::hex_encode(&util::random_bytes::<8>()?)));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .ctx(|| format!("creating {}", tmp.display()))?;
    let part = PartFile { path: tmp.clone(), keep: false };
    let counted = AtomicU64::new(0);
    let byte_progress = (progress.unit() == Unit::Bytes).then_some((progress, &counted));
    let mut sink = FileSink { file, hasher: Hasher::new(art.expect.alg), written: 0, progress: byte_progress };

    let result = async {
        let limit = art.size.unwrap_or(default_limit);
        let status = conn.call(Method::GET, path, &Vec::new(), None, limit, &mut sink).await?.status;
        if status != 200 {
            return Err(Error::Net(format!("{}: HTTP {status}", art.url)));
        }
        if let Some(size) = art.size
            && size != sink.written
        {
            return Err(Error::Integrity(format!("{}: received {} bytes, expected {size}", art.name, sink.written)));
        }
        let FileSink { file, hasher, .. } = sink;
        let digest = hasher.finish();
        if !art.expect.matches(&digest) {
            return Err(Error::Integrity(format!(
                "{}: downloaded {} is {}, published digest is {} (from {})",
                art.name,
                art.expect.alg.name(),
                util::hex_encode(&digest),
                art.expect.hex(),
                art.url
            )));
        }
        file.sync_all().ctx(|| format!("syncing {}", tmp.display()))?;
        drop(file); // fs-verity cannot be enabled while a writable descriptor exists
        // Kernel-enforced immutability where available; read-only mode bits everywhere.
        if let Ok(frozen) = File::open(&tmp) {
            let _ = crate::verity::enable(&frozen);
        }
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o444)).ctx(|| "setting permissions")?;
        fs::rename(&tmp, &stored.path).ctx(|| format!("installing {}", stored.path.display()))?;
        Ok(())
    }
    .await;
    match &result {
        // Renamed into place: there is no temporary file any more.
        Ok(()) => part.keep(),
        Err(_) => progress.retract(counted.load(Ordering::Relaxed)),
    }
    result
}

// ---------------------------------------------------------------- legacy asset layout

/// Versions before 1.7.3 look assets up by logical name. Build that view out of hard links to
/// the content-addressed objects (same inode, so it is covered by the same verification).
fn build_logical_tree(index: &AssetIndex, assets_root: &Path, root: &Path) -> Result<()> {
    util::create_private_dir_all(root)?;
    for (name, obj) in &index.objects {
        validate_relative(Path::new(name))?;
        let hash = Expected::sha1(&obj.hash)?;
        let source = assets_root.join("objects").join(&hash.hex()[..2]).join(hash.hex());
        let target = root.join(name);
        if let Some(parent) = target.parent() {
            util::create_private_dir_all(parent)?;
        }
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_file() && meta.len() == obj.size => continue,
            Ok(_) => fs::remove_file(&target).ctx(|| format!("replacing {}", target.display()))?,
            Err(_) => {}
        }
        if fs::hard_link(&source, &target).is_err() {
            fs::copy(&source, &target).ctx(|| format!("copying asset to {}", target.display()))?;
        }
    }
    Ok(())
}

fn verify_logical_tree(index: &AssetIndex, root: &Path) -> Result<()> {
    for (name, obj) in &index.objects {
        let path = root.join(name);
        let file = util::open_regular_nofollow(&path).ctx(|| format!("opening {}", path.display()))?;
        Expected::sha1(&obj.hash)?.verify_file(&file, Some(obj.size), &format!("asset {name}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- natives (pre-1.19)

/// Unpack shared objects from a verified natives jar into `dest` (a fresh, launcher-owned
/// directory). Entry names are flattened to their final component and must look like a
/// shared object, so a hostile archive cannot write outside `dest` or drop other file types.
pub fn extract_natives(jar: &File, exclude: &[String], dest: &Path) -> Result<usize> {
    let mut reader = jar.try_clone().ctx(|| "duplicating natives jar descriptor")?;
    reader.seek(SeekFrom::Start(0)).ctx(|| "rewinding natives jar")?;
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("natives jar is not a valid zip: {e}")))?;
    let mut extracted = 0usize;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| Error::Format(format!("natives jar entry {i}: {e}")))?;
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }
        let name = entry.name().map_err(|e| Error::Format(format!("natives jar entry {i}: {e}")))?.into_owned();
        if exclude.iter().any(|prefix| name.starts_with(prefix.as_str())) {
            continue;
        }
        let base = name.rsplit('/').next().unwrap_or_default();
        let is_shared_object = base.ends_with(".so") || base.contains(".so.");
        if !is_shared_object || validate_file_name(base).is_err() {
            continue;
        }
        let out_path = dest.join(base);
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&out_path)
            .ctx(|| format!("creating {}", out_path.display()))?;
        let copied =
            io::copy(&mut (&mut entry).take(NATIVE_FILE_LIMIT + 1), &mut out).ctx(|| format!("extracting {base}"))?;
        total += copied;
        if copied > NATIVE_FILE_LIMIT || total > NATIVE_TOTAL_LIMIT {
            return Err(Error::Policy(format!("natives jar entry {base} exceeds the extraction size limit")));
        }
        extracted += 1;
    }
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::HashAlg;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join(format!("bastion-test-{tag}-{}", util::hex_encode(&util::random_bytes::<6>().unwrap())));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unfinished_downloads_remove_themselves_and_stale_ones_are_swept() {
        let dir = tmpdir("parts");
        let make = |name: &str| {
            let path = dir.join(name);
            fs::write(&path, b"partial").unwrap();
            path
        };
        // Dropped without `keep` (an error, or a cancelled download): gone.
        let abandoned = make(".part-0000000000000001");
        drop(PartFile { path: abandoned.clone(), keep: false });
        assert!(!abandoned.exists());
        // Kept (it was renamed into place; here the name simply stays): untouched.
        let finished = make(".part-0000000000000002");
        PartFile { path: finished.clone(), keep: false }.keep();
        assert!(finished.exists());

        // The sweep only takes leftovers nobody can still be writing, and only leftovers.
        let fresh = make(".part-0000000000000003");
        let old = make(".part-0000000000000004");
        let unrelated = make("lwjgl-3.3.6.jar");
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
        for path in [&old, &unrelated] {
            fs::File::options().write(true).open(path).unwrap().set_modified(long_ago).unwrap();
        }
        sweep_stale_parts(&dir);
        assert!(fresh.exists(), "a download that may still be running is left alone");
        assert!(!old.exists(), "a leftover from a killed run is removed");
        assert!(unrelated.exists(), "stored files are never swept, whatever their age");
        sweep_stale_parts(&dir.join("does-not-exist"));
        fs::remove_dir_all(dir).unwrap();
    }

    fn artifact(rel: &str, content: &[u8]) -> Artifact {
        Artifact {
            name: rel.to_string(),
            rel_path: rel.to_string(),
            url: format!("https://libraries.minecraft.net/{rel}"),
            expect: Expected::sha1(&util::hex_encode(&util::hash_bytes(HashAlg::Sha1, content))).unwrap(),
            size: Some(content.len() as u64),
        }
    }

    #[test]
    fn classify_distinguishes_ok_missing_and_tampered() {
        let root = tmpdir("classify");
        fs::write(root.join("good.jar"), b"good").unwrap();
        fs::write(root.join("bad.jar"), b"tampered!").unwrap();
        fs::write(root.join("samesize.jar"), b"evil").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("link.jar")).unwrap();
        let items = vec![
            Stored::new(&root, artifact("good.jar", b"good")).unwrap(),
            Stored::new(&root, artifact("bad.jar", b"original")).unwrap(),
            Stored::new(&root, artifact("samesize.jar", b"good")).unwrap(),
            Stored::new(&root, artifact("absent.jar", b"x")).unwrap(),
            Stored::new(&root, artifact("link.jar", b"x")).unwrap(),
        ];
        let c = classify(items, 4).unwrap();
        assert_eq!(c.missing.len(), 1);
        assert_eq!(c.missing[0].artifact.rel_path, "absent.jar");
        let corrupt: Vec<_> = c.corrupt.iter().map(|(s, _)| s.artifact.rel_path.as_str()).collect();
        assert_eq!(
            corrupt,
            vec!["bad.jar", "samesize.jar", "link.jar"],
            "symlinks and modified files are tampering, not 'missing'"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stored_rejects_escaping_paths() {
        let root = tmpdir("escape");
        assert!(Stored::new(&root, artifact("../outside.jar", b"x")).is_err());
        assert!(Stored::new(&root, artifact("/abs.jar", b"x")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn natives_extraction_is_confined() {
        use std::io::Cursor;
        let root = tmpdir("natives");
        let jar_path = root.join("n.jar");
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in [
                ("liblwjgl64.so", &b"ELF-one"[..]),
                ("META-INF/MANIFEST.MF", b"manifest"),
                ("../../escape.so", b"ELF-escape"),
                ("nested/dir/libnested.so", b"ELF-nested"),
                ("readme.txt", b"not a library"),
                ("/abs/libabs.so.1", b"ELF-abs"),
            ] {
                zw.start_file(name, opts).unwrap();
                zw.write_all(data).unwrap();
            }
            fs::write(&jar_path, zw.finish().unwrap().into_inner()).unwrap();
        }
        let dest = root.join("out");
        fs::create_dir(&dest).unwrap();
        let n = extract_natives(&File::open(&jar_path).unwrap(), &["META-INF/".to_string()], &dest).unwrap();
        let mut names: Vec<_> =
            fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        // Everything lands flat inside `dest`; nothing escapes, non-libraries are skipped.
        assert_eq!(names, vec!["escape.so", "libabs.so.1", "liblwjgl64.so", "libnested.so"]);
        assert_eq!(n, 4);
        assert!(!root.join("escape.so").exists() && !root.parent().unwrap().join("escape.so").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
