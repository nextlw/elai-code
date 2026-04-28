use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Resolves the base directory for task output files.
///
/// Priority: `ELAI_TASK_DIR` env var → `$HOME/.elai/tasks` → `./.elai/tasks`.
fn task_base_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ELAI_TASK_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".elai").join("tasks")
}

/// Path para output de uma task: `~/.elai/tasks/<id>.log` (ou `$ELAI_TASK_DIR/<id>.log`).
#[must_use]
pub fn task_output_path(task_id: &str) -> PathBuf {
    task_output_path_in(&task_base_dir(), task_id)
}

/// Constrói o path de output dado um diretório base explícito.
/// Exposto para uso em testes sem mutação de ambiente.
#[must_use]
pub fn task_output_path_in(base: &Path, task_id: &str) -> PathBuf {
    base.join(format!("{task_id}.log"))
}

pub struct TaskOutputWriter {
    path: PathBuf,
    file: File,
    bytes_written: u64,
}

impl TaskOutputWriter {
    pub fn open(task_id: &str) -> io::Result<Self> {
        Self::open_in(&task_base_dir(), task_id)
    }

    /// Cria ou abre o arquivo de output em um diretório base explícito.
    pub fn open_in(base: &Path, task_id: &str) -> io::Result<Self> {
        let path = task_output_path_in(base, task_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let bytes_written = std::fs::metadata(&path).map_or(0, |m| m.len());
        Ok(Self {
            path,
            file,
            bytes_written,
        })
    }

    pub fn write_chunk(&mut self, data: &[u8]) -> io::Result<u64> {
        self.file.write_all(data)?;
        self.bytes_written = self.bytes_written.saturating_add(data.len() as u64);
        Ok(data.len() as u64)
    }

    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Lê output a partir de um offset. Útil para streaming via UI.
pub fn read_output_from(task_id: &str, offset: u64) -> io::Result<Vec<u8>> {
    read_output_from_in(&task_base_dir(), task_id, offset)
}

/// Lê output a partir de um offset em um diretório base explícito.
pub fn read_output_from_in(base: &Path, task_id: &str, offset: u64) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let path = task_output_path_in(base, task_id);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let mut file = File::open(&path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_output_path_includes_id() {
        let id = "btest1234";
        let path = task_output_path(id);
        assert!(
            path.to_string_lossy().contains(id),
            "path should contain id: {}",
            path.display()
        );
        assert!(path.to_string_lossy().ends_with(".log"));
    }

    #[test]
    fn output_writer_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let task_id = "btest5678";

        let mut writer = TaskOutputWriter::open_in(base, task_id).unwrap();
        let n = writer.write_chunk(b"hello").unwrap();
        assert_eq!(n, 5);
        assert_eq!(writer.bytes_written(), 5);

        let data = read_output_from_in(base, task_id, 0).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn read_from_nonexistent_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let data = read_output_from_in(tmp.path(), "does_not_exist_xyz999", 0).unwrap();
        assert!(data.is_empty());
    }
}
