pub use tokio::fs::{File, OpenOptions, ReadDir};

pub async fn read(path: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

pub async fn write(
    path: impl AsRef<std::path::Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    tokio::fs::write(path, contents).await
}

pub async fn create_dir_all(path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await
}

pub async fn metadata(path: impl AsRef<std::path::Path>) -> std::io::Result<std::fs::Metadata> {
    tokio::fs::metadata(path).await
}

pub async fn read_dir(path: impl AsRef<std::path::Path>) -> std::io::Result<ReadDir> {
    tokio::fs::read_dir(path).await
}
