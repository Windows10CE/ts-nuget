use std::{sync::Arc, time::Duration};

use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use warp::*;

type WarpResult<T> = Result<T, Rejection>;

#[derive(Deserialize, Serialize)]
struct Resource {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "@type")]
    pub res_type: String,
}

const BASE_URL: &str = "http://localhost:5000";

mod metadata;
use crate::metadata::Cache;

mod nupkg;
use crate::nupkg::Nupkg;

static METADATA: OnceCell<Arc<RwLock<Cache>>> = OnceCell::new();

#[tokio::main]
async fn main() {
    match std::fs::create_dir("nupkgs") {
        Ok(_) => (),
        Err(e) => match e.kind() {
            std::io::ErrorKind::AlreadyExists => (),
            _ => panic!(),
        },
    }

    let meta = METADATA.get_or_init(|| Arc::new(RwLock::new(Cache::default())));

    tokio::task::spawn_blocking(|| Cache::cache(meta))
        .await
        .unwrap();

    Cache::enable_auto_update(meta.clone(), Duration::from_secs(60)).await;

    let get_services = path!("nuget" / "v3" / "index.json")
        .and(get().or(head()))
        .and_then(move |_| async move {
            let services = vec![
                Resource {
                    id: format!("{BASE_URL}/nuget/v3/base"),
                    res_type: "PackageBaseAddress/3.0.0".to_string(),
                },
                Resource {
                    id: format!("{BASE_URL}/nuget/v3/search"),
                    res_type: "SearchQueryService".to_string(),
                },
                Resource {
                    id: format!("{BASE_URL}/nuget/v3/nullpublish"),
                    res_type: "PackagePublish/2.0.0".to_string(),
                },
                Resource {
                    id: format!("{BASE_URL}/nuget/v3/package"),
                    res_type: "RegistrationsBaseUrl".to_string(),
                },
            ];

            let json = json!({
                "version": "3.0.0",
                "resources": services,
            });

            let res: WarpResult<_> = Ok(reply::json(&json));
            res
        })
        .with(filters::compression::gzip());

    let package_base = path!("nuget" / "v3" / "base" / String / "index.json")
        .and(get().or(head()))
        .and_then(move |pkg: String, _| async move {
            let cache = METADATA.get().unwrap().read();
            let res: WarpResult<_> = match cache.packages.get(&pkg.to_lowercase()) {
                Some(pkg) => {
                    let versions: Vec<&str> = pkg.items[0]
                        .items
                        .iter()
                        .map(|x| x.catalogEntry.version.as_str())
                        .collect();
                    Ok(reply::json(&json!({ "versions": versions })))
                }
                None => Err(reject::not_found()),
            };

            res
        });

    let package_download = path!("nuget" / "v3" / "base" / String / String / String).and_then(
        move |pkg, ver, _| async move {
            let cache = METADATA.get().unwrap();
            let nuget = cache.read().packages.get(&pkg).unwrap().items[0]
                .items
                .iter()
                .find(|x| x.catalogEntry.version == ver)
                .unwrap()
                .clone();
            let res: WarpResult<_> = Ok(reply::Response::new(
                Nupkg::get_for_pkg(&nuget).await.into(),
            ));
            res
        },
    );

    let reg_base = path!("nuget" / "v3" / "package" / String / "index.json")
        .and(get().or(head()))
        .and_then(move |full_name: String, _| async move {
            let res: WarpResult<_>;

            let cache = METADATA.get().unwrap().read();
            if let Some(pkg) = cache.packages.get(&full_name.to_lowercase()) {
                res = Ok(reply::json(pkg))
            } else {
                res = Err(warp::reject::not_found());
            }

            res
        })
        .with(filters::compression::gzip());

    let search = path!("nuget" / "v3" / "search")
        .and(get().or(head()))
        .and_then(move |_| async move {
            let res: WarpResult<_> = Ok(reply::json(&METADATA.get().unwrap().read().search(None)));
            res
        });

    let all = get_services
        .or(package_base)
        .or(package_download)
        .or(reg_base)
        .or(search);

    let logged = all.with(log::custom(|x| println!("{}: {}", x.method(), x.path())));

    println!("ready!");

    warp::serve(logged).run(([127, 0, 0, 1], 5000)).await;
}
