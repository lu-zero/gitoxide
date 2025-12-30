use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// A minimalist implementation of `git add`.
use clap::Parser;
use gix::bstr::ByteSlice;

fn main() {
    let args = Args::parse_from(gix::env::args_os());
    match run(args) {
        Ok(()) => {}
        Err(e) => eprintln!("error: {e}"),
    }
}

#[derive(Debug, clap::Parser)]
#[clap(name = "add", about = "git add example", version = option_env!("GIX_VERSION"))]
#[clap(arg_required_else_help = true)]
struct Args {
    /// Alternative git directory to use
    #[clap(name = "dir", long = "git-dir")]
    git_dir: Option<PathBuf>,
    /// Files to add to the index
    #[clap(name = "pathspec", required = true)]
    pathspecs: Vec<PathBuf>,
}

fn run(args: Args) -> anyhow::Result<()> {
    let repo = gix::discover(args.git_dir.as_deref().unwrap_or(Path::new(".")))?;
    let worktree_root = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("Need a worktree to add files"))?;

    // Open the index for modification
    let mut index = repo.open_index()?;

    for pathspec in args.pathspecs {
        // Resolve the path relative to the worktree
        let abs_path = if pathspec.is_absolute() {
            pathspec.clone()
        } else {
            std::env::current_dir()?.join(&pathspec)
        };

        // Canonicalize both paths to handle symlinks and ensure proper comparison
        let abs_path = abs_path.canonicalize()?;
        let worktree_canonical = worktree_root.canonicalize()?;

        // Ensure the path is within the worktree
        let rel_path = abs_path
            .strip_prefix(&worktree_canonical)
            .map_err(|_| anyhow::anyhow!("Path '{}' is outside repository", abs_path.display()))?;

        // Read the file and write it as a blob
        let mut file = std::fs::File::open(&abs_path)?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        let blob_id = repo.write_blob(&contents)?;

        // Get file metadata
        let metadata = gix::index::fs::Metadata::from_path_no_follow(&abs_path)?;
        let stat = gix::index::entry::Stat::from_fs(&metadata)?;

        // Determine file mode
        let mode = if metadata.is_symlink() {
            gix::index::entry::Mode::SYMLINK
        } else if metadata.is_executable() {
            gix::index::entry::Mode::FILE_EXECUTABLE
        } else {
            gix::index::entry::Mode::FILE
        };

        // Create flags for stage 0 (unconflicted)
        let flags = gix::index::entry::Flags::from_stage(gix::index::entry::Stage::Unconflicted);

        // Convert the path to bytes
        let path_bytes = gix::path::os_str_into_bstr(rel_path.as_os_str())?.to_owned();

        // Check if an entry already exists for this path
        if let Some(existing_idx) = index
            .entry_index_by_path_and_stage(path_bytes.as_bstr(), gix::index::entry::Stage::Unconflicted)
        {
            // Update the existing entry
            let entry = &mut index.entries_mut()[existing_idx];
            entry.stat = stat;
            entry.id = blob_id.detach();
            entry.mode = mode;
            println!("Updated: {}", rel_path.display());
        } else {
            // Add a new entry
            index.dangerously_push_entry(stat, blob_id.detach(), flags, mode, path_bytes.as_bstr());
            println!("Added: {}", rel_path.display());
        }
    }

    // Sort entries to maintain index invariants
    index.sort_entries();

    // Write the index back to disk
    index.write(Default::default())?;

    Ok(())
}
