use std::{
    io::{Read, Write},
    path::{self, Path, PathBuf},
};
use axum::body::StreamBody;
use axum::response::IntoResponse;

use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use zip::{write::FileOptions, ZipArchive, ZipWriter};

use crate::metadata::NugetVersion;

pub struct Nupkg {
    path: PathBuf,
}

impl Nupkg {
    pub async fn get_for_pkg(pkg: &NugetVersion) -> Result<Self, reqwest::Error> {
        let name = format!("{}.{}", pkg.catalogEntry.id, pkg.catalogEntry.version);
        let init_path = path::Path::new("nupkgs");
        let path = init_path.join(name.clone() + ".nupkg");
        let zip_path = init_path.join(name + ".zip");

        if !path.exists() {
            let ts_bytes = reqwest::get(&pkg.catalogEntry.download_url)
                .await?
                .bytes()
                .await?;

            let mut zip_file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&zip_path)
                .await
                .unwrap();
            zip_file.write_all(&ts_bytes).await.unwrap();
            let mut zip = ZipArchive::new(zip_file.into_std().await).unwrap();

            let mut nuget = ZipWriter::new(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .open(&path)
                    .unwrap(),
            );
            let names: Vec<String> = zip
                .file_names()
                .filter(|x| x.ends_with(".dll"))
                .map(|x| x.to_string())
                .collect();
            for file in names {
                nuget
                    .start_file(
                        Path::new("lib")
                            .join("netstandard2.0")
                            .join(Path::new(&file).file_name().unwrap())
                            .to_string_lossy(),
                        FileOptions::default(),
                    )
                    .unwrap();
                let mut inner_file = zip.by_name(&file).unwrap();
                let mut bytes: Vec<u8> = Vec::with_capacity(inner_file.size() as _);
                inner_file.read_to_end(&mut bytes).unwrap();
                nuget.write_all(&bytes).unwrap();
            }

            nuget
                .start_file(
                    format!("{}.nuspec", pkg.catalogEntry.id),
                    FileOptions::default(),
                )
                .unwrap();
            nuget
                .write_all(
                    format!(
                        include_str!("template.nuspec"),
                        pkg.catalogEntry.id, pkg.catalogEntry.version, pkg.catalogEntry.description
                    )
                    .as_bytes(),
                )
                .unwrap();

            drop(zip);
            tokio::fs::remove_file(zip_path).await.unwrap();
        }

        Ok(Self { path })
    }

    pub async fn get_body(&self) -> impl IntoResponse {
        StreamBody::new(ReaderStream::new(File::open(&self.path).await.unwrap()))
    }
}
