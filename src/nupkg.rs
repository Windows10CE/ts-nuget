use axum::body::Body;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

use crate::metadata::NugetVersion;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub struct Nupkg {
    path: PathBuf,
}

impl Nupkg {
    pub async fn get_for_pkg(pkg: &NugetVersion) -> Result<Self, reqwest::Error> {
        let name = format!("{}.{}", pkg.catalogEntry.id, pkg.catalogEntry.version);
        let init_path = Path::new("nupkgs");
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
                .truncate(true)
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
                    .truncate(true)
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
                    .start_file_from_path(
                        Path::new("lib")
                            .join("netstandard2.0")
                            .join(Path::new(&file).file_name().unwrap()),
                        SimpleFileOptions::default(),
                    )
                    .unwrap();
                let mut inner_file = zip.by_name(&file).unwrap();
                std::io::copy(&mut inner_file, &mut nuget).unwrap();
            }

            nuget
                .start_file(
                    format!("{}.nuspec", pkg.catalogEntry.id),
                    SimpleFileOptions::default(),
                )
                .unwrap();

            write!(
                nuget,
                include_str!("template.nuspec"),
                pkg.catalogEntry.id, pkg.catalogEntry.version, pkg.catalogEntry.description
            )
            .unwrap();

            drop(zip);
            tokio::fs::remove_file(zip_path).await.unwrap();
        }

        Ok(Self { path })
    }

    pub async fn get_body(&self) -> Body {
        Body::from_stream(ReaderStream::new(File::open(&self.path).await.unwrap()))
    }
}
