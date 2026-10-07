use base64::Engine;
use serde::{Serialize,Deserialize};
use sha2::{Digest,Sha256};
use std::{fs::{File,OpenOptions},io::Read,path::Path};
use crate::update_transaction::{Payload,Version};
type Result<T> = std::result::Result<T,String>;
pub fn read_bounded(path:&Path,limit:usize)->Result<Vec<u8>> {
    let file=File::open(path).map_err(|_|"Cannot open signed metadata")?;
    let mut bytes=Vec::new();file.take(limit as u64+1).read_to_end(&mut bytes).map_err(|_|"Cannot read signed metadata")?;
    if bytes.len()>limit{return Err("Signed metadata limit exceeded".into());}Ok(bytes)
}
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema:u32,pub version:String,pub build:String,pub platform:String,pub asset:String,pub url:String,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub installer_kind:Option<String>,
    pub size:u64,pub sha256:String,pub signature:String,pub helper_protocol:u32,pub files:Vec<Payload>,
}
fn decode(encoded:&str)->Result<String> {
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|_|"Invalid signature encoding")?).map_err(|_|"Invalid signature text".into())
}
pub struct VerifiedManifest(Manifest);
impl VerifiedManifest {pub fn data(&self)->&Manifest {&self.0}}
pub fn manifest(bytes:&[u8],signature:&str,public_key:&str)->Result<VerifiedManifest> {
    if bytes.len()>1024*1024 || signature.len()>4096 || public_key.len()>4096 {return Err("Signed metadata limit exceeded".into());}
    let key=minisign_verify::PublicKey::decode(&decode(public_key)?).map_err(|_|"Invalid verification key")?;
    let signature=minisign_verify::Signature::decode(&decode(signature)?).map_err(|_|"Invalid manifest signature")?;
    key.verify(bytes,&signature,false).map_err(|_|"Manifest signature failed")?;
    let value:Manifest=serde_json::from_slice(bytes).map_err(|_|"Invalid signed manifest")?;
    let url=url::Url::parse(&value.url).map_err(|_|"Invalid artifact URL")?;
    if ![1,2].contains(&value.schema) || (value.schema==2 && value.installer_kind.as_deref()!=Some("transactional-v1")) || value.platform!="windows-x86_64" || value.helper_protocol!=1 || value.size==0 ||
        url.scheme()!="https" || url.host_str()!=Some("github.com") || url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() ||
        value.asset.contains(['/','\\',':']) || value.asset.is_empty() || !url.path().ends_with(&format!("/{}",value.asset)) ||
        value.sha256.len()!=64 || !value.sha256.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("Unsupported signed update identity".into());}
    if url.path()!=format!("/snaps-creator/atlas/releases/download/v{}/{}",value.version,value.asset) {return Err("Unexpected update repository or version URL".into());}
    value.payload()?.validate()?;Ok(VerifiedManifest(value))
}
impl Manifest {
    pub fn payload(&self)->Result<Version> {
        let value=Version{id:format!("{}-{}",self.version,self.build),version:self.version.clone(),build:self.build.clone(),files:self.files.clone(),absent:false};value.validate()?;
        for required in ["Atlas.exe","Atlas.Service.exe","AtlasMaintenance.exe","libcef.dll","resources/Atlas.Core.exe","resources/Atlas.Xray.exe"] {
            if !value.files.iter().any(|f|f.path==required){return Err("Signed payload is incomplete".into());}
        }
        let ui=value.files.iter().find(|f|f.path=="Atlas.exe").unwrap();let service=value.files.iter().find(|f|f.path=="Atlas.Service.exe").unwrap();
        if ui.sha256!=service.sha256 || ui.size!=service.size{return Err("UI/service payload identity mismatch".into());}Ok(value)
    }
}
/// Keeps the verified artifact open without write/delete sharing. The bytes
/// cannot be exchanged between verification and subsequent staging/execution.
pub struct VerifiedPackage { pub manifest:VerifiedManifest, _file:File }
impl VerifiedPackage {
    pub fn open(path:&Path,verified:VerifiedManifest,public_key:&str)->Result<Self> {
        let manifest=verified.data();
        let key=minisign_verify::PublicKey::decode(&decode(public_key)?).map_err(|_|"Invalid verification key")?;
        let signature=minisign_verify::Signature::decode(&decode(&manifest.signature)?).map_err(|_|"Invalid artifact signature")?;
        let mut verifier=key.verify_stream(&signature).map_err(|_|"Artifact requires modern minisign signature")?;
        let mut options=OpenOptions::new();options.read(true);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(1);}
        let mut file=options.open(path).map_err(|_|"Cannot pin update artifact")?;
        let mut buffer=[0u8;65536];let mut size=0u64;let mut hash=Sha256::new();
        loop {let count=file.read(&mut buffer).map_err(|_|"Cannot read update artifact")?;if count==0{break;}size+=count as u64;if size>manifest.size{return Err("Artifact size mismatch".into());}hash.update(&buffer[..count]);verifier.update(&buffer[..count]);}
        if size!=manifest.size || format!("{:x}",hash.finalize())!=manifest.sha256 {return Err("Artifact size/hash mismatch".into());}
        verifier.finalize().map_err(|_|"Artifact signature failed")?;
        Ok(Self{manifest:verified,_file:file})
    }
}

#[cfg(test)]mod tests {
    use super::*;
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../testdata/update-crypto.json")).unwrap()}
    fn bytes(v:&serde_json::Value,key:&str)->Vec<u8>{base64::engine::general_purpose::STANDARD.decode(v[key].as_str().unwrap()).unwrap()}
    #[test]fn metadata_is_bounded_before_allocation(){let p=std::env::temp_dir().join(format!("atlas-metadata-{}",uuid::Uuid::new_v4()));std::fs::write(&p,b"12345").unwrap();assert!(read_bounded(&p,4).is_err());assert_eq!(read_bounded(&p,5).unwrap(),b"12345");std::fs::remove_file(p).unwrap();}
    #[test]fn native_signature_positive_and_negative_cases() {
        let v=fixture();let key=v["publicKey"].as_str().unwrap();let sig=v["signature"].as_str().unwrap();let data=bytes(&v,"manifest");
        assert!(manifest(&data,sig,key).is_ok());let mut bad=data.clone();bad[0]^=1;assert!(manifest(&bad,sig,key).is_err());assert!(manifest(&data,"AAAA",key).is_err());assert!(manifest(&data,sig,"AAAA").is_err());
        assert!(manifest(&bytes(&v,"incompatibleManifest"),v["incompatibleSignature"].as_str().unwrap(),key).is_err());
    }
    #[test]fn pinned_exact_artifact_cannot_be_replaced_and_corruption_fails() {
        let v=fixture();let key=v["publicKey"].as_str().unwrap();let sig=v["signature"].as_str().unwrap();let path=std::env::temp_dir().join(format!("atlas-signature-{}",uuid::Uuid::new_v4()));
        std::fs::write(&path,bytes(&v,"artifact")).unwrap();let verified=manifest(&bytes(&v,"manifest"),sig,key).unwrap();
        let pinned=VerifiedPackage::open(&path,verified,key).unwrap();assert!(std::fs::write(&path,b"bad").is_err());assert!(std::fs::remove_file(&path).is_err());drop(pinned);
        let mut bad=bytes(&v,"artifact");bad[0]^=1;std::fs::write(&path,bad).unwrap();assert!(VerifiedPackage::open(&path,manifest(&bytes(&v,"manifest"),sig,key).unwrap(),key).is_err());std::fs::remove_file(path).unwrap();
    }
}
