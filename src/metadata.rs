use std::{collections::HashMap, sync::Arc, time::Duration};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub struct Cache {
    auto_update: Option<Arc<CancellationToken>>,
    pub packages: HashMap<String, NugetPackage>,
}

impl Cache {
    pub fn cache(cache: &RwLock<Cache>) {
        let mut c = cache.write();

        c.packages.clear();

        let ts: Vec<TSPackage> = reqwest::blocking::get("https://thunderstore.io/api/v1/package/")
            .unwrap()
            .json()
            .unwrap();

        c.packages.shrink_to(ts.len());

        for package in ts {
            c.packages
                .insert(package.full_name.clone().to_lowercase(), package.into());
        }
    }

    pub async fn enable_auto_update(cache: Arc<RwLock<Cache>>, timeout: Duration) {
        let mut s = cache.write();

        Cache::disable_auto_update(&mut s);

        let arc = cache.clone();
        let token = Arc::new(CancellationToken::new());
        let thread_token = token.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(timeout).await;
                if thread_token.is_cancelled() {
                    return;
                }

                let arc_inner = arc.clone();
                tokio::task::spawn_blocking(move || Cache::cache(&arc_inner))
                    .await
                    .unwrap();
            }
        });

        s.auto_update = Some(token);
    }

    pub fn disable_auto_update(cache: &mut Cache) {
        let update = match &cache.auto_update {
            Some(x) => x,
            None => return,
        };

        update.cancel();

        cache.auto_update = None;
    }

    pub fn search(&self, _q: Option<String>) -> SearchResult {
        let results: Vec<_> = self.packages.values().map(|x| x.into()).collect();
        SearchResult {
            totalHits: results.len(),
            data: results,
        }
    }
}

impl Default for Cache {
    fn default() -> Self {
        Cache {
            auto_update: None,
            packages: HashMap::new(),
        }
    }
}

#[derive(Deserialize)]
pub struct TSPackage {
    pub full_name: String,
    pub package_url: String,
    pub date_updated: String,
    pub is_deprecated: bool,
    pub versions: Vec<TSVersion>,
}

#[derive(Deserialize)]
pub struct TSVersion {
    pub description: String,
    pub icon: String,
    pub version_number: String,
    pub download_url: String,
    pub downloads: u32,
    pub date_created: String,
    pub website_url: String,
    pub file_size: u64,
}

#[derive(Serialize)]
pub struct NugetPackage {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "@type")]
    pub res_type: [String; 3],
    pub count: u8,
    pub items: [NugetPackageInner; 1],
}

#[derive(Serialize)]
pub struct NugetPackageInner {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(skip)]
    pub full_name: String,
    pub count: usize,
    pub lower: String,
    pub upper: String,
    pub items: Vec<NugetVersion>,
}

#[allow(non_snake_case)]
#[derive(Serialize, Clone)]
pub struct NugetVersion {
    #[serde(rename = "@id")]
    pub id: String,
    pub packageContent: String,
    pub catalogEntry: NugetVersionInner,
}

#[allow(non_snake_case)]
#[derive(Serialize, Clone)]
pub struct NugetVersionInner {
    pub id: String,
    pub description: String,
    pub iconUrl: String,
    pub published: String,
    pub version: String,
    pub packageContent: String,
    #[serde(skip)]
    pub downloads: u32,
    #[serde(skip)]
    pub download_url: String,
}

impl From<TSPackage> for NugetPackage {
    fn from(pkg: TSPackage) -> Self {
        let url = format!(
            "{}/nuget/v3/package/{}/index.json",
            crate::BASE_URL,
            pkg.full_name.to_lowercase()
        );

        NugetPackage {
            id: url.clone(),
            res_type: [
                "PackageRegistration".to_string(),
                "catalog:CatalogRoot".to_string(),
                "catalog:Permalink".to_string(),
            ],
            count: 1,
            items: [NugetPackageInner {
                id: url.clone(),
                full_name: pkg.full_name.clone(),
                count: pkg.versions.len(),
                lower: pkg.versions.last().unwrap().version_number.clone(),
                upper: pkg.versions.first().unwrap().version_number.clone(),
                items: pkg
                    .versions
                    .into_iter()
                    .map(|version| NugetVersion {
                        id: url.clone(),
                        packageContent: format!(
                            "{}/nuget/v3/base/{}/{}/{}.{}.nupkg",
                            crate::BASE_URL,
                            pkg.full_name.to_lowercase(),
                            version.version_number,
                            pkg.full_name.to_lowercase(),
                            version.version_number
                        ),
                        catalogEntry: NugetVersionInner {
                            id: pkg.full_name.clone(),
                            description: version.description,
                            iconUrl: version.icon,
                            published: version.date_created,
                            packageContent: format!(
                                "{}/nuget/v3/base/{}/{}/{}.{}.nupkg",
                                crate::BASE_URL,
                                pkg.full_name.to_lowercase(),
                                version.version_number,
                                pkg.full_name.to_lowercase(),
                                version.version_number
                            ),
                            version: version.version_number,
                            downloads: version.downloads,
                            download_url: version.download_url,
                        },
                    })
                    .collect(),
            }],
        }
    }
}

#[allow(non_snake_case)]
#[derive(Serialize)]
pub struct SearchResult {
    pub totalHits: usize,
    pub data: Vec<SearchItem>,
}

#[allow(non_snake_case)]
#[derive(Serialize)]
pub struct SearchItem {
    pub id: String,
    pub version: String,
    pub description: String,
    pub versions: Vec<SearchVersion>,
    pub iconUrl: String,
    pub registration: String,
}

impl From<&NugetPackage> for SearchItem {
    fn from(pkg: &NugetPackage) -> Self {
        Self {
            id: pkg.items[0].full_name.clone(),
            version: pkg.items[0].upper.clone(),
            description: pkg.items[0].items[0].catalogEntry.description.clone(),
            versions: pkg.items[0].items.iter().map(|x| x.into()).collect(),
            iconUrl: pkg.items[0].items[0].catalogEntry.iconUrl.clone(),
            registration: format!(
                "{}/nuget/v3/package/{}/index.json",
                crate::BASE_URL,
                pkg.items[0].full_name
            ),
        }
    }
}

#[derive(Serialize)]
pub struct SearchVersion {
    #[serde(rename = "@id")]
    pub id: String,
    pub version: String,
    pub downloads: u32,
}

impl From<&NugetVersion> for SearchVersion {
    fn from(ver: &NugetVersion) -> Self {
        Self {
            id: "".to_string(),
            version: ver.catalogEntry.version.clone(),
            downloads: ver.catalogEntry.downloads,
        }
    }
}
