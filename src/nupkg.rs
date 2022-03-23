use std::{path::{PathBuf, self}, fs::File, io::Read};

use hyper::Body;

use crate::metadata::NugetVersion;

pub struct Nupkg {
    path: PathBuf,
}

impl Nupkg {
    pub fn get_for_pkg(pkg: &NugetVersion) -> Self {
        let path = path::Path::new("nupkgs").join(format!("{}.{}.nupkg", pkg.catalogEntry.id, pkg.catalogEntry.version));
        println!("{:#?}", path);
        Self {
            path
        }
    }
}

impl Into<Body> for Nupkg {
    fn into(self) -> Body {
        File::open(self.path).unwrap().bytes().filter_map(|x| x.ok()).collect::<Vec<u8>>().into()
    }
}
