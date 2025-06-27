use axum::body::Bytes;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::{sync::Arc, time::Duration};
use tokio::{sync::RwLock, time::Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::Router;
use tokio::net::TcpListener;

#[derive(Serialize)]
struct Resource {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "@type")]
    pub res_type: String,
}

static BASE_URL: OnceLock<String> = OnceLock::new();

mod metadata;

use crate::metadata::{Cache, PackageKey, SearchQuery};

mod nupkg;

use crate::nupkg::Nupkg;

type SharedState = Arc<RwLock<Cache>>;

const DEFAULT_CACHE: Duration = Duration::from_secs(5 * 60);

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

    let shared_state: SharedState = Default::default();

    let cache_start = Instant::now();
    Cache::cache(&shared_state)
        .await
        .expect("Failed to get cache");
    println!(
        "Took {} seconds to get full cache",
        cache_start.elapsed().as_secs_f64()
    );

    Cache::enable_auto_update(shared_state.clone(), DEFAULT_CACHE).await;

    let app = Router::new()
        .route("/nuget/v3/index.json", axum::routing::get(get_services))
        .route(
            "/nuget/v3/base/{id}/index.json",
            axum::routing::get(get_base),
        )
        .route(
            "/nuget/v3/base/{id}/{ver}/{filename}",
            axum::routing::get(get_download),
        )
        .route(
            "/nuget/v3/package/{id}/index.json",
            axum::routing::get(get_registry),
        )
        .route("/nuget/v3/search", axum::routing::get(search))
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(shared_state.clone());

    let rt = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        for _ in std::io::stdin().lines() {
            rt.block_on(Cache::cache(&shared_state)).unwrap();
            println!("forced cache refresh");
        }
    });

    axum::serve(TcpListener::bind(("0.0.0.0", port)).await.unwrap(), app)
        .await
        .unwrap()
}

async fn get_services() -> Json<Value> {
    let url = BASE_URL.get().unwrap();

    let resources = [
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

    Json(json!({
        "version": "3.0.0",
        "resources": resources,
    }))
}

async fn get_base(
    Path(id): Path<String>,
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, StatusCode> {
    let cache = state.read().await;

    cache
        .packages
        .get(&PackageKey::try_from(id).map_err(|_| StatusCode::BAD_REQUEST)?)
        .map(|package| {
            let versions = package.items[0]
                .items
                .iter()
                .map(|version| version.catalogEntry.version.as_str())
                .collect::<Vec<_>>();
            (
                [(
                    "Cache-Control",
                    format!(
                        "max-age={}",
                        cache.cache_duration.unwrap_or(DEFAULT_CACHE).as_secs() / 2
                    ),
                )],
                Json(json!({ "versions": versions })),
            )
        })
        .ok_or(StatusCode::NOT_FOUND)
}

async fn get_download(
    Path((id, ver, _)): Path<(String, String, ())>,
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, StatusCode> {
    let version = state
        .read()
        .await
        .packages
        .get(&PackageKey::try_from(id).map_err(|_| StatusCode::BAD_REQUEST)?)
        .and_then(|pkg| {
            pkg.items[0]
                .items
                .iter()
                .find(|nuget_ver| nuget_ver.catalogEntry.version == ver)
        })
        .ok_or(StatusCode::NOT_FOUND)?
        .clone();

    let response = Nupkg::get_for_pkg(&version)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .get_body()
        .await;

    Ok(([("Cache-Control", "max-age=1209600, immutable")], response))
}

async fn get_registry(
    Path(id): Path<String>,
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, StatusCode> {
    let cache = state.read().await;

    cache
        .packages
        .get(&PackageKey::try_from(id).map_err(|_| StatusCode::BAD_REQUEST)?)
        .map(|pkg| {
            (
                [(
                    "Cache-Control",
                    format!(
                        "max-age={}",
                        cache.cache_duration.unwrap_or(DEFAULT_CACHE).as_secs() / 2
                    ),
                )],
                Json(serde_json::to_value(pkg).unwrap()),
            )
        })
        .ok_or(StatusCode::NOT_FOUND)
}

enum SearchResponse {
    All(Bytes),
    Query(Json<metadata::SearchResult>),
}

impl IntoResponse for SearchResponse {
    fn into_response(self) -> Response {
        match self {
            SearchResponse::All(all) => {
                ([(header::CONTENT_TYPE, "application/json")], all).into_response()
            }
            SearchResponse::Query(query) => query.into_response(),
        }
    }
}

async fn search(
    Query(params): Query<SearchQuery>,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let cache = state.read().await;

    let body = if matches!(
        params,
        SearchQuery {
            query: None,
            skip: None,
            take: None
        }
    ) {
        SearchResponse::All(cache.all_packages.clone())
    } else {
        SearchResponse::Query(Json(cache.search(params)))
    };

    (
        [(
            "Cache-Control",
            format!(
                "max-age={}",
                cache.cache_duration.unwrap_or(DEFAULT_CACHE).as_secs() / 2
            ),
        )],
        body,
    )
}
