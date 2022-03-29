use std::{sync::Arc, time::Duration};

use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::json;
use warp::*;

type WarpResult<T> = Result<T, Rejection>;

#[derive(Serialize)]
struct Resource {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "@type")]
    pub res_type: String,
}

static BASE_URL: OnceCell<String> = OnceCell::new();

mod metadata;
use crate::metadata::{Cache, SearchQuery};

mod nupkg;
use crate::nupkg::Nupkg;

static METADATA: OnceCell<Arc<RwLock<Cache>>> = OnceCell::new();

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();

    BASE_URL
        .set(std::env::var("NUGET_BASE_URL").expect("Needs NUGET_BASE_URL"))
        .unwrap();
    let port: u16 = match std::env::var("NUGET_PORT") {
        Ok(port_str) => match port_str.parse() {
            Ok(p) => p,
            Err(_) => panic!("Couldn't parse port"),
        },
        Err(_) => panic!("Needs NUGET_PORT"),
    };

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

    Cache::enable_auto_update(meta.clone(), Duration::from_secs(60 * 5)).await;

    let get_services = path!("nuget" / "v3" / "index.json")
        .and(get().or(head()))
        .and_then(move |_| async move {
            let url = BASE_URL.get().unwrap();
            let services = vec![
                Resource {
                    id: format!("{url}/nuget/v3/base"),
                    res_type: "PackageBaseAddress/3.0.0".to_string(),
                },
                Resource {
                    id: format!("{url}/nuget/v3/search"),
                    res_type: "SearchQueryService".to_string(),
                },
                Resource {
                    id: format!("{url}/nuget/v3/search"),
                    res_type: "SearchQueryService/3.0.0-beta".to_string(),
                },
                Resource {
                    id: format!("{url}/nuget/v3/search"),
                    res_type: "SearchQueryService/3.0.0-rc".to_string(),
                },
                Resource {
                    id: format!("{url}/nuget/v3/nullpublish"),
                    res_type: "PackagePublish/2.0.0".to_string(),
                },
                Resource {
                    id: format!("{url}/nuget/v3/package"),
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
        .and(query::<SearchQuery>())
        .and_then(move |_, params: SearchQuery| async move {
            let results = &METADATA.get().unwrap().read().search(params);
            let res: WarpResult<_> = Ok(reply::json(&results));
            res
        });

    let all = get_services
        .or(package_base)
        .or(package_download)
        .or(reg_base)
        .or(search);

    #[cfg(debug_assertions)]
    let final_filter = all.with(log::custom(|x| println!("{}: {}", x.method(), x.path())));

    #[cfg(not(debug_assertions))]
    let final_filter = all;

    println!("ready!");

    warp::serve(final_filter).run(([0, 0, 0, 0], port)).await;
}
