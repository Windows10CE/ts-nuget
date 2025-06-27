use axum::body::Bytes;
use futures::{pin_mut, FutureExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

mod key {
    use std::borrow::Cow;
    use std::hash::Hash;
    use thiserror::Error;

    pub struct PackageKey<'a>(Cow<'a, str>);
    #[derive(Error, Debug, Clone)]
    #[error("Package name was not ASCII; {0}")]
    pub struct NonAsciiError<'a>(Cow<'a, str>);

    impl<'a> TryFrom<&'a str> for PackageKey<'a> {
        type Error = NonAsciiError<'a>;

        fn try_from(value: &'a str) -> Result<Self, Self::Error> {
            if value.is_ascii() {
                Ok(Self(Cow::Borrowed(value)))
            } else {
                Err(NonAsciiError(Cow::Borrowed(value)))
            }
        }
    }

    impl TryFrom<String> for PackageKey<'_> {
        type Error = NonAsciiError<'static>;

        fn try_from(value: String) -> Result<Self, Self::Error> {
            if value.is_ascii() {
                Ok(Self(Cow::Owned(value)))
            } else {
                Err(NonAsciiError(Cow::Owned(value)))
            }
        }
    }

    impl Hash for PackageKey<'_> {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            for b in self.0.as_bytes() {
                state.write_u8(b.to_ascii_lowercase());
            }
            state.write_u8(0xff);
        }
    }

    impl PartialEq<PackageKey<'_>> for PackageKey<'_> {
        fn eq(&self, other: &PackageKey<'_>) -> bool {
            self.0.eq_ignore_ascii_case(&other.0)
        }
    }

    impl Eq for PackageKey<'_> {}
}

pub use key::*;

#[derive(Default)]
pub struct Cache {
    auto_update: Option<Arc<CancellationToken>>,
    pub cache_duration: Option<Duration>,
    pub packages: HashMap<PackageKey<'static>, NugetPackage>,
    pub all_packages: Bytes,
}

impl Cache {
    pub async fn cache(cache: &RwLock<Cache>) -> Result<(), reqwest::Error> {
        let mut next_option =
            Some("https://thunderstore.io/api/experimental/community/".to_string());
        let mut communities = vec![];

        while let Some(next) = next_option {
            let list = reqwest::get(next).await?.json::<TSCommunityList>().await?;
            communities.extend(list.results.into_iter().map(|x| x.identifier));
            next_option = list.pagination.next_link;
        }

        let packages = futures::future::join_all(communities.into_iter().map(|comm| async move {
            reqwest::get(format!("https://thunderstore.io/c/{comm}/api/v1/package/"))
                .await?
                .json::<Vec<TSPackage>>()
                .await
        }))
        .await;

        let packages: HashMap<_, _> = packages
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .map(|p| {
                (
                    PackageKey::try_from(p.full_name.clone()).unwrap(),
                    NugetPackage::from(p),
                )
            })
            .collect();

        let all_package_string = serde_json::to_string(&SearchResult {
            totalHits: packages.len(),
            data: packages.values().map(|p| p.into()).collect(),
        })
        .unwrap();

        let mut cache = cache.write().await;

        cache.packages = packages;
        cache.all_packages = all_package_string.into();

        Ok(())
    }

    pub async fn enable_auto_update(cache: Arc<RwLock<Cache>>, timeout: Duration) {
        let mut s = cache.write().await;

        if s.auto_update.is_some() {
            return;
        }
        let token = Arc::new(CancellationToken::new());
        s.auto_update = Some(token.clone());
        s.cache_duration = Some(timeout);

        drop(s);

        tokio::spawn(async move {
            let cancel_future = token.cancelled().fuse();
            pin_mut!(cancel_future);
            loop {
                futures::select_biased! {
                    _ = cancel_future => return,
                    _ = tokio::time::sleep(timeout).fuse() => match Cache::cache(&cache).await {
                        Ok(_) => (),
                        Err(err) => eprintln!("Unexpected error while updating cache! {err:?}"),
                    },
                }
            }
        });
    }

    #[allow(dead_code)]
    pub fn disable_auto_update(cache: &mut Cache) {
        if let Some(token) = cache.auto_update.take() {
            token.cancel();
        }
    }

    pub fn search(&self, q: SearchQuery) -> SearchResult {
        let mut results: &mut dyn Iterator<Item = &NugetPackage> = &mut self.packages.values();

        let mut search_results;
        let mut skip_results;
        let mut take_results;

        if let Some(query) = q.query {
            let lowercase = query.to_lowercase();
            search_results =
                results.filter(move |x| x.items[0].full_name_lower.contains(&lowercase));
            results = &mut search_results;
        }

        if let Some(skip) = q.skip {
            skip_results = results.skip(skip);
            results = &mut skip_results;
        }

        if let Some(take) = q.take {
            take_results = results.take(take);
            results = &mut take_results;
        }

        let v: Vec<_> = results.map(|x| x.into()).collect();

        SearchResult {
            totalHits: v.len(),
            data: v,
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
pub struct Pagination {
    pub next_link: Option<String>,
}

#[derive(Deserialize)]
pub struct TSCommunity {
    pub identifier: String,
}

#[derive(Deserialize)]
pub struct TSCommunityList {
    pub pagination: Pagination,
    pub results: Vec<TSCommunity>,
}

#[derive(Deserialize)]
pub struct TSPackage {
    pub full_name: String,
    pub package_url: String,
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
    pub dependencies: Vec<String>,
}

#[derive(Serialize)]
pub struct NugetPackage {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "@type")]
    pub res_type: [&'static str; 3],
    pub count: u8,
    pub items: [NugetPackageInner; 1],
}

#[derive(Serialize)]
pub struct NugetPackageInner {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(skip)]
    pub full_name: String,
    #[serde(skip)]
    pub full_name_lower: String,
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

#[derive(Serialize, Clone)]
pub struct Deprecation {
    #[serde(rename = "@id")]
    pub id: String,
    pub message: &'static str,
    pub reasons: [&'static str; 1],
}

#[allow(non_snake_case)]
#[derive(Serialize, Clone)]
pub struct NugetVersionInner {
    #[serde(rename = "@id")]
    pub id: String,
    pub description: String,
    pub iconUrl: String,
    pub published: String,
    pub version: String,
    pub packageContent: String,
    pub deprecation: Option<Deprecation>,
    #[serde(skip)]
    pub downloads: u32,
    #[serde(skip)]
    pub download_url: String,
}

impl From<TSPackage> for NugetPackage {
    fn from(pkg: TSPackage) -> Self {
        let base_url = crate::BASE_URL.get().unwrap();
        let full_name_lower = pkg.full_name.to_lowercase();
        let url = format!(
            "{}/nuget/v3/package/{}/index.json",
            base_url, full_name_lower
        );

        NugetPackage {
            id: url.clone(),
            res_type: [
                "PackageRegistration",
                "catalog:CatalogRoot",
                "catalog:Permalink",
            ],
            count: 1,
            items: [NugetPackageInner {
                id: url.clone(),
                full_name: pkg.full_name.clone(),
                full_name_lower: full_name_lower.clone(),
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
                            full_name_lower,
                            version.version_number,
                            full_name_lower,
                            version.version_number
                        ),
                        catalogEntry: NugetVersionInner {
                            id: pkg.full_name.clone(),
                            description: [&format!(
                                "{}\n\nPackage URL: {}\nWebsite URL: {}\nDepends on:",
                                version.description, pkg.package_url, version.website_url
                            )]
                            .into_iter()
                            .chain(&version.dependencies)
                            .map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                            iconUrl: version.icon,
                            published: version.date_created,
                            packageContent: format!(
                                "{}/nuget/v3/base/{}/{}/{}.{}.nupkg",
                                base_url,
                                full_name_lower,
                                version.version_number,
                                full_name_lower,
                                version.version_number
                            ),
                            version: version.version_number,
                            downloads: version.downloads,
                            download_url: version.download_url,
                            deprecation: pkg.is_deprecated.then(|| Deprecation {
                                id: format!("{url}#deprecation"),
                                message: "Deprecated on Thunderstore",
                                reasons: ["Other"],
                            }),
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
            id: format!("{}#{}", ver.id, ver.catalogEntry.version),
            version: ver.catalogEntry.version.clone(),
            downloads: ver.catalogEntry.downloads,
        }
    }
}
