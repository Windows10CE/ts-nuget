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

static METADATA: OnceCell<Arc<RwLock<Cache>>> = OnceCell::new();

#[tokio::main]
async fn main() {
    let meta = METADATA.get_or_init(|| Arc::new(RwLock::new(Cache::default())));

    tokio::task::spawn_blocking(|| Cache::cache(meta)).await.unwrap();

    Cache::enable_auto_update(meta.clone(), Duration::from_secs(60)).await;

    let get_services = 
        path!("nuget" / "v3" / "index.json")
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

    let package_base =
        path!("nuget" / "v3" / "download" / String / "index.json")
        .and(get().or(head()))
        .and_then(move |pkg: String, _| async move {
            let cache = METADATA.get().unwrap().read();
            let versions: Vec<&str> = cache.packages.get(&pkg).unwrap().items[0].items.iter().map(|x| x.catalogEntry.version.as_str()).collect();

            let res: WarpResult<_> = Ok(reply::json(&json!({
                "versions": versions
            })));
            res
        });

    let reg_base =
        path!("nuget" / "v3" / "package" / String / "index.json")
        .and(get().or(head()))
        .and_then(move |full_name: String, _| async move {
            let res: WarpResult<_>;

            let cache = METADATA.get().unwrap().read();
            if let Some(pkg) = cache.packages.get(&full_name.to_lowercase()) {
                res = Ok(reply::json(pkg))
            }
            else {
                res = Err(warp::reject::not_found());
            }

            res
        })
        .with(filters::compression::gzip());

    let search =
        path!("nuget" / "v3" / "search")
        .and(get().or(head()))
        .and_then(move |_| async move {
            let res: WarpResult<_> = Ok(reply::json(&METADATA.get().unwrap().read().search(None)));
            res
        });

    let all =
        get_services
        .or(package_base)
        .or(reg_base)
        .or(search);

    let logged = all.with(log::custom(|x| println!("{}: {}", x.method(), x.path())));

    warp::serve(logged).run(([127, 0, 0, 1], 5000)).await;
}
