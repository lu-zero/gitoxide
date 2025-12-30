use std::{
    fs,
    path::{Path, PathBuf},
};

/// A toy-version of `git add`.
use clap::Parser;
use gix::{bstr::BStr, index};

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
    /// Add files to the index only, without considering what to commit
    #[clap(short, long)]
    intent_to_add: bool,
    /// Files to add
    #[clap(name = "files")]
    files: Vec<PathBuf>,
}

fn run(args: Args) -> anyhow::Result<()> {
    let repo = gix::discover(args.git_dir.as_deref().unwrap_or(Path::new(".")))?;

    // Open the index or create an empty one if it doesn't exist
    let index_snapshot = repo.index_or_empty()?;

    // Get the underlying index file and its state
    let index_file = &*index_snapshot;
    let state = &**index_file; // Dereference to get the State

    let mut state = state.clone();

    for file_path in &args.files {
        // Convert to repository-relative path
        let repo_relative_path = if file_path.is_absolute() {
            // Try to strip the workdir prefix, handling both /tmp and /private/tmp cases
            let workdir = repo.workdir().unwrap();
            match file_path.strip_prefix(workdir) {
                Ok(stripped) => stripped.to_path_buf(),
                Err(_) => {
                    // If direct strip fails, try with normalized paths
                    let normalized_workdir = fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
                    let normalized_path = fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
                    match normalized_path.strip_prefix(&normalized_workdir) {
                        Ok(stripped) => stripped.to_path_buf(),
                        Err(_) => file_path.to_path_buf(),
                    }
                }
            }
        } else {
            file_path.to_path_buf()
        };

        let file_path_to_read = repo.workdir().unwrap().join(&repo_relative_path);

        // Get metadata to determine file type (use the original path, not canonicalized)
        let symlink_metadata = fs::symlink_metadata(&file_path_to_read)?;

        // Check if the file exists
        if !file_path_to_read.exists() {
            eprintln!("Path '{}' does not exist", repo_relative_path.display());
            continue;
        }

        let canonical_path = fs::canonicalize(&file_path_to_read)?;

        // Determine the mode based on file type
        let mode = if symlink_metadata.is_symlink() {
            index::entry::Mode::SYMLINK
        } else if symlink_metadata.is_dir() {
            eprintln!("Path '{}' is a directory, skipping", repo_relative_path.display());
            continue;
        } else {
            // Check if executable
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if symlink_metadata.permissions().mode() & 0o111 != 0 {
                    index::entry::Mode::FILE_EXECUTABLE
                } else {
                    index::entry::Mode::FILE
                }
            }
            #[cfg(not(unix))]
            {
                index::entry::Mode::FILE
            }
        };

        // Read file content or symlink target and create a blob object
        let file_content = if symlink_metadata.is_symlink() {
            // For symlinks, read the target path
            fs::read_link(&file_path_to_read)?
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            // For regular files, read the content
            fs::read(&canonical_path)?
        };
        let blob_id = repo.write_blob(&file_content)?;

        // Convert path to repository-relative BStr
        let repo_relative_bstr = BStr::new(repo_relative_path.as_os_str().as_encoded_bytes());

        // Add entry to the index
        let flags = if args.intent_to_add {
            index::entry::Flags::INTENT_TO_ADD
        } else {
            index::entry::Flags::empty()
        };

        state.dangerously_push_entry(
            index::entry::Stat::default(),
            blob_id.detach(),
            flags,
            mode,
            repo_relative_bstr,
        );

        println!("add '{}'", repo_relative_path.display());
    }

    // Sort entries to maintain index invariants
    state.sort_entries();

    // Write the updated index back to disk
    let mut index_file = index::File::from_state(state.into(), repo.index_path());
    index_file.write(index::write::Options::default())?;

    Ok(())
}
