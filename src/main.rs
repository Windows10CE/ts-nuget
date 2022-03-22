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
        .and(get())
        .and_then(move || async move {
            let services = vec![
                Resource {
                    id: format!("{BASE_URL}/nuget/v3/package"),
                    res_type: "PackageBaseAddress/3.6.0".to_string(),
                },
                Resource {
                    id: format!("{BASE_URL}/nuget/v3/search"),
                    res_type: "SearchQueryService".to_string(),
                },
            ];

            let json = json!({
                "version": "3.0.0",
                "resources": services,
            });

            let res: WarpResult<_> = Ok(reply::json(&json));
            res
        });

    let package_base =
        path!("nuget" / "v3" / "package" / String / "index.json")
        .and_then(move |full_name: String| async move {
            let res: WarpResult<_>;

            let cache = METADATA.get().unwrap().read();
            if let Some(pkg) = cache.packages.get(&full_name.to_lowercase()) {
                res = Ok(reply::json(pkg))
            }
            else {
                res = Err(warp::reject::not_found());
            }

            println!("{:#?}", full_name);
            res
        })
        .with(filters::compression::gzip());

    let search =
        path!("nuget" / "v3" / "search")
        .and_then(move || async move {
            let res: WarpResult<_> = Ok(reply::json(&METADATA.get().unwrap().read().search(None)));
            res
        })
        .with(filters::compression::gzip());

    let all =
        get_services
        .or(package_base)
        .or(search);

    warp::serve(all).run(([0, 0, 0, 0], 5000)).await;
}
