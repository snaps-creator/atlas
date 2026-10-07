//! Manual installers authenticate their embedded payload with a receipt built
//! into the native installer helper. The helper binds its own bytes separately,
//! avoiding a recursive self-hash. Network updates still require signed metadata.
use crate::update_transaction::{Version,Payload,digest};
use std::path::Path;
pub fn payload(source:&Path)->Result<Version,String>{
    let receipt=include_bytes!(concat!(env!("OUT_DIR"),"/install-receipt.json"));
    let mut version:Version=serde_json::from_slice(receipt).map_err(|_|"This updater has no embedded installation payload")?;
    version.validate()?;
    if version.absent || version.version!=env!("CARGO_PKG_VERSION") || version.build!=env!("ATLAS_BUILD_ID") {
        return Err("Embedded installation payload has a different build identity".into());
    }
    if version.files.iter().any(|f|f.path.eq_ignore_ascii_case("AtlasUpdater.exe")) {
        return Err("Installation receipt must not contain a recursive updater hash".into());
    }
    let self_path=std::env::current_exe().map_err(|e|e.to_string())?;
    let (size,sha256)=digest(&self_path)?;
    version.files.push(Payload{path:"AtlasUpdater.exe".into(),size,sha256});
    version.files.sort_by(|a,b|a.path.cmp(&b.path));
    for required in ["Atlas.exe","Atlas.Service.exe","AtlasMaintenance.exe","libcef.dll","resources/Atlas.Core.exe","resources/Atlas.Xray.exe"] {
        if !version.files.iter().any(|f|f.path==required){return Err(format!("Installation payload is missing {required}"));}
    }
    let ui=version.files.iter().find(|f|f.path=="Atlas.exe").unwrap();
    let service=version.files.iter().find(|f|f.path=="Atlas.Service.exe").unwrap();
    if ui.size!=service.size||ui.sha256!=service.sha256{return Err("Installation UI and service differ".into());}
    version.verify(source)?;
    Ok(version)
}
