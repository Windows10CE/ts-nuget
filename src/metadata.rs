use std::{collections::HashMap, io::Write, sync::Arc, time::Duration};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub struct Cache {
    auto_update: Option<Arc<CancellationToken>>,
    pub packages: HashMap<String, NugetPackage>,
}

impl Cache {
    pub fn cache(cache: &RwLock<Cache>) -> Result<(), reqwest::Error> {
        let mut c = cache.write();

        c.packages.clear();

        let mut ts: Vec<TSPackage> = Vec::new();

        for subdomain in include_str!("subdomains.txt").lines() {
            ts.append(
                &mut reqwest::blocking::get(format!(
                    "https://{subdomain}thunderstore.io/api/v1/package/"
                ))?
                .json::<Vec<TSPackage>>()?,
            );
        }

        c.packages.shrink_to(ts.len());

        for package in ts {
            if !c.packages.contains_key(&package.full_name) {
                c.packages
                    .insert(package.full_name.clone().to_lowercase(), package.into());
            }
        }

        Ok(())
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
                match tokio::task::spawn_blocking(move || Cache::cache(&arc_inner))
                    .await
                    .unwrap()
                {
                    Ok(_) => (),
                    Err(err) => writeln!(
                        std::io::stderr(),
                        "Unexpected error while updating cache! {:?}",
                        err
                    )
                    .unwrap(),
                }
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

    pub fn search(&self, q: SearchQuery) -> SearchResult {
        let results = self.packages.values().map(|x| x.into());

        let mut final_vec: Vec<_> = match q.query {
            Some(param) => {
                let lowercase = param.to_lowercase();
                results
                    .filter(|x: &SearchItem| x.id.to_lowercase().contains(&lowercase))
                    .collect()
            }
            None => results.collect(),
        };

        if let Some(skip) = q.skip {
            final_vec = final_vec.into_iter().skip(skip).collect();
        }

        if let Some(take) = q.take {
            final_vec = final_vec.into_iter().take(take).collect();
        }

        SearchResult {
            totalHits: final_vec.len(),
            data: final_vec,
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

#[derive(Deserialize, Debug)]
pub struct SearchQuery {
    #[serde(rename = "q")]
    pub query: Option<String>,
    pub skip: Option<usize>,
    pub take: Option<usize>,
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
    pub dependencies: Vec<String>,
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
        let base_url = crate::BASE_URL.get().unwrap();
        let url = format!(
            "{}/nuget/v3/package/{}/index.json",
            base_url,
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
                            base_url,
                            pkg.full_name.to_lowercase(),
                            version.version_number,
                            pkg.full_name.to_lowercase(),
                            version.version_number
                        ),
                        catalogEntry: NugetVersionInner {
                            id: pkg.full_name.clone(),
                            description: [format!("{}\n\nDepends on:", version.description)]
                                .iter()
                                .chain(&version.dependencies)
                                .map(|x| x.as_str())
                                .collect::<Vec<&str>>()
                                .join("\n"),
                            iconUrl: version.icon,
                            published: version.date_created,
                            packageContent: format!(
                                "{}/nuget/v3/base/{}/{}/{}.{}.nupkg",
                                base_url,
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
#[derive(Serialize, Debug)]
pub struct SearchResult {
    pub totalHits: usize,
    pub data: Vec<SearchItem>,
}

#[allow(non_snake_case)]
#[derive(Serialize, Debug)]
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
                crate::BASE_URL.get().unwrap(),
                pkg.items[0].full_name
            ),
        }
    }
}

#[derive(Serialize, Debug)]
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
