use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;

struct Codemark {
    db_path: PathBuf,
    binary: PathBuf,
    temp_dir: Option<tempfile::TempDir>,
    work_dir: PathBuf,
}

impl Codemark {
    fn with_git_repo() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo_path = temp.path().to_path_buf();

        let repo = git2::Repository::init(&repo_path).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();

        let codemark_dir = repo_path.join(".codemark");
        std::fs::create_dir_all(&codemark_dir).unwrap();
        let db_path = codemark_dir.join("codemark.db");

        let binary = PathBuf::from(env!("CARGO_BIN_EXE_codemark"));

        Codemark {
            db_path,
            binary,
            temp_dir: Some(temp),
            work_dir: repo_path,
        }
    }

    fn commit(&self, file_path: &str, content: &str, message: &str) -> String {
        let full_path = self.work_dir.join(file_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full_path, content).unwrap();

        let repo = git2::Repository::open(&self.work_dir).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(file_path)).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo.signature().unwrap();

        let head_oid = match repo.head() {
            Ok(head) => {
                let head_commit = head.peel_to_commit().unwrap();
                Some(head_commit.id())
            }
            Err(_) => None,
        };

        let oid = repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            message,
            &tree,
            head_oid.as_ref().map(|id| &id[..]).unwrap_or(&[]),
        ).unwrap();
        oid.to_string()
    }

    fn file_path(&self, file: &str) -> String {
        self.work_dir.join(file).to_string_lossy().to_string()
    }

    fn run(&self, args: &[&str]) -> (String, String, i32) {
        let output = Command::new(&self.binary)
            .arg("--db")
            .arg(&self.db_path)
            .args(args)
            .current_dir(&self.work_dir)
            .output()
            .expect("failed to run codemark");

        (String::from_utf8_lossy(&output.stdout).to_string(),
         String::from_utf8_lossy(&output.stderr).to_string(),
         output.status.code().unwrap_or(-1))
    }
}

fn main() {
    let cm = Codemark::with_git_repo();

    // Create initial code
    cm.commit("test.rs", "fn my_function() { let x = 1; }", "Initial");

    // Create bookmark
    let (stdout, _stderr, status) = cm.run(&["add", "--file", &cm.file_path("test.rs"), "--range", "1", "--format", "json"]);
    println!("Add status: {}", status);
    println!("Add stdout: {}", stdout);

    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let id = json["data"]["id"].as_str().unwrap();
    println!("Bookmark ID: {}", id);

    // Try to show the bookmark
    let (show_stdout, show_stderr, show_status) = cm.run(&["show", &id[..8], "--format", "json"]);
    println!("\nShow status: {}", show_status);
    println!("Show stdout: {}", show_stdout);
    println!("Show stderr: {}", show_stderr);
}
