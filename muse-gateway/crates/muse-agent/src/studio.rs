//! Muse Studio: durable scene projects, reusable references, and resumable
//! background media jobs. All project artifacts stay in the agent workspace.
//! No provider secrets appear in tool receipts or project files.
use std::{collections::HashSet, path::{Path, PathBuf}, sync::{LazyLock, Mutex}};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::{process::Command, time::{sleep, Duration}};
use uuid::Uuid;

static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
const MAX_SCENES: usize = 24;
const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 160 * 1024 * 1024;

#[derive(Clone)]
pub struct MediaGateway {
    port: u16,
    key: String,
    client: reqwest::Client,
}
impl MediaGateway {
    pub fn new(port: u16, key: String) -> Result<Self, String> {
        if port == 0 || key.is_empty() { return Err("Media gateway configuration invalid".into()); }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(18))
            .build().map_err(|e| e.to_string())?;
        Ok(Self { port, key, client })
    }
    fn endpoint(&self, suffix: &str) -> String {
        format!("http://127.0.0.1:{}/v1/{}", self.port, suffix)
    }
    async fn request(&self, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = self.endpoint(path);
        let req = if let Some(value) = body {
            self.client.post(url).header("content-type","application/json").body(value.to_string())
        } else {
            self.client.get(url)
        };
        let res = req.bearer_auth(&self.key).send().await
            .map_err(|e| format!("Gateway media unavailable: {e}"))?;
        let status = res.status();
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 500_000 { return Err("Media response exceeds JSON limit".into()); }
        if !status.is_success() {
            return Err(format!("Gateway media request HTTP {status}: {}",String::from_utf8_lossy(&bytes).chars().take(250).collect::<String>()));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("Unexpected media response: {e}"))
    }
    pub async fn video(&self, prompt: &str, duration: u32, aspect: &str, frame: Option<&str>) -> Result<String, String> {
        let mut body = json!({"prompt":prompt,"duration":duration,"size":aspect});
        if let Some(frame) = frame { body["image"] = json!(frame); }
        let value = self.request("videos",Some(body)).await?;
        value.get("id").and_then(Value::as_str).map(str::to_string)
            .ok_or_else(|| "Media gateway did not return a video task id".into())
    }
    pub async fn image(&self, prompt: &str, aspect: &str, frame: Option<&str>) -> Result<String, String> {
        let mut body = json!({"prompt":prompt,"size":aspect,"async":true,"n":1,"response_format":"url"});
        if let Some(frame)=frame { body["image"] = json!([frame]); }
        let value=self.request("images/generations",Some(body)).await?;
        value.get("id").and_then(Value::as_str).map(str::to_string)
            .ok_or_else(|| "Media gateway did not return an image task id".into())
    }
    pub async fn task(&self, kind:&str,id:&str)->Result<Value,String>{
        if !safe_token(id) {return Err("Invalid media task id".into());}
        self.request(&format!("{kind}/{id}"),None).await
    }
    pub async fn asset(&self, url:&str)->Result<(Vec<u8>,String),String>{
        // Never fetch untrusted provider URLs directly. Gateway normally
        // returns a RELATIVE /v1/media/file.mp4 when PUBLIC_BASE is empty.
        // Also support fully qualified URLs for configured deployments.
        // In both cases only the validated basename is fetched from the
        // local gateway media store.
        let name=media_result_name(url)?;
        let path=format!("media/{name}");
        let resp=self.client.get(self.endpoint(&path)).send().await.map_err(|e|e.to_string())?;
        if !resp.status().is_success(){return Err(format!("Media download HTTP {}",resp.status()));}
        if resp.content_length().unwrap_or(0)>MAX_MEDIA_BYTES as u64{return Err("Media file too large".into());}
        let mime=resp.headers().get("content-type").and_then(|s|s.to_str().ok()).unwrap_or("application/octet-stream").to_string();
        let bytes=resp.bytes().await.map_err(|e|e.to_string())?;
        if bytes.len()>MAX_MEDIA_BYTES{return Err("Media exceeds size limit".into());}
        Ok((bytes.to_vec(),mime))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    pub description: String,
    pub reference_image: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub description: String,
    pub reference_audio: Option<String>,
    pub permission_confirmed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Scene {
    pub number: u32,
    pub duration: u32,
    pub prompt: String,
    #[serde(default)]
    pub dialogue: String,
    #[serde(default)]
    pub character_ids: Vec<String>,
    pub voice_id: Option<String>,
    pub reference_image: Option<String>,
    pub image_task: Option<String>,
    pub image_asset: Option<String>,
    pub video_task: Option<String>,
    pub video_asset: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct StudioProject {
    pub id: String,
    pub name: String,
    pub style: String,
    pub aspect: String,
    pub created_at: String,
    pub updated_at: String,
    pub characters: Vec<Character>,
    pub voices: Vec<Voice>,
    pub scenes: Vec<Scene>,
    pub final_video: Option<String>,
    pub status: String,
}

/// Prepare one straightforward image or single-scene video request using
/// local metadata only. No model/browser response is needed before the
/// normal explicit external-media approval screen.
///
/// Complex projects (multi-scene, storyboards, imported references) still
/// use the full Muse planner; this fast path must never guess their assets.
pub fn quick_plan(prompt: &str) -> Option<(Map<String,Value>, &'static str)> {
    let raw=prompt.trim();
    if raw.is_empty() || raw.len()>1200 || raw.contains('\n') || raw.starts_with('/') {
        return None;
    }
    let has=|pattern:&str|regex::Regex::new(pattern).ok().is_some_and(|re|re.is_match(raw));
    if !has(r"(?i)\b(?:buat(?:kan|lah)?|bikin|buatkan|generate|create|make|hasilkan)\b")
        || !has(r"(?i)\b(?:video|film|animasi|gambar|image|picture|foto)\b")
        || has(r"(?i)\b(?:script|skrip|python|kode|code|program|aplikasi|website|tutorial|cara|bagaimana|apakah|bisakah)\b")
        || has(r"(?i)\b(?:scene|adegan|episode|storyboard|gabung|gabungkan|combine|lanjutkan|folder|project|proyek|referensi|reference|lokasi|suara|voice)\b")
    {
        return None;
    }
    let video=has(r"(?i)\b(?:video|film|animasi)\b");
    let duration=if video {
        let re=regex::Regex::new(r"(?i)\b(\d{1,2})\s*(?:detik|seconds?|sec|s)\b").ok()?;
        let secs=re.captures(raw)?.get(1)?.as_str().parse::<u32>().ok()?;
        if !(1..=60).contains(&secs){return None}
        secs
    } else {10};
    let id=format!("muse-{}-{}",if video{"video"}else{"image"},&uuid::Uuid::new_v4().simple().to_string()[..10]);
    let name=raw.chars().take(110).collect::<String>();
    let scene_prompt=if video {
        format!("{raw}\nCreate exactly one continuous scene, duration {duration} seconds. Preserve the user's requested subjects and visual style.")
    } else {
        format!("{raw}\nProduce the requested still image, preserving the user's exact subject and style.")
    };
    let style=if has(r"(?i)\b3d\b") {"3D"} else {""};
    let aspect=if raw.contains("9:16") {"9:16"} else if raw.contains("1:1") {"1:1"} else {"16:9"};
    let value=json!({
        "name":name,"id":id,"scene_count":1,"scene_duration":duration,
        "style":style,"aspect":aspect,"scene_prompts":[scene_prompt]
    });
    Some((value.as_object()?.clone(),if video{"studio_render_project"}else{"studio_render_image"}))
}
fn media_result_name(url:&str)->Result<String,String>{
    let path=if url.starts_with("/v1/media/"){
        if url.contains('?') || url.contains('#'){return Err("Media path must not include query or fragment".into())}
        url.to_string()
    }else{
        let parsed=reqwest::Url::parse(url).map_err(|_|"Malformed media URL")?;
        if !matches!(parsed.scheme(),"http"|"https"){
            return Err("Media URL must be http(s) or a local media path".into());
        }
        if parsed.query().is_some() || parsed.fragment().is_some(){
            return Err("Media URL query/fragment not supported".into());
        }
        parsed.path().to_string()
    };
    let name=path.strip_prefix("/v1/media/")
        .ok_or("Media URL is not a gateway /v1/media/ path")?;
    if !safe_token(name) || !name.contains('.') || name.contains('/') {
        return Err("Invalid media file name".into());
    }
    Ok(name.to_string())
}
fn safe_token(v:&str)->bool{
    !v.is_empty() && v.len()<=100 && v.bytes().all(|c| c.is_ascii_alphanumeric()|| matches!(c,b'.'|b'-'|b'_'))
        && v!="." && v!=".." && !v.contains("..")
}
fn slug(s:&str)->Result<String,String>{
    let v:String=s.to_ascii_lowercase().chars()
        .map(|c|if c.is_ascii_alphanumeric() {c} else {'-'}).collect();
    let v=v.trim_matches('-').to_string();
    if v.is_empty() || v.len()>48 || !safe_token(&v){return Err("Project/asset id must contain 1-48 ASCII letters or numbers".into());}
    Ok(v)
}
fn str_arg<'a>(a:&'a Map<String,Value>,key:&str)->&'a str{
    a.get(key).and_then(Value::as_str).unwrap_or("")
}
fn number(a:&Map<String,Value>,key:&str,default:u32)->u32{
    a.get(key).and_then(Value::as_u64).and_then(|n|u32::try_from(n).ok()).unwrap_or(default)
}
fn timestamp()->String{chrono::Utc::now().to_rfc3339()}
fn base(root:&Path)->PathBuf{root.join(".muse-studio")}
fn dir(root:&Path,id:&str)->Result<PathBuf,String>{
    if !safe_token(id) || id.len()>48{return Err("Invalid studio project id".into());}
    let parent=base(root);
    if parent.exists() && parent.symlink_metadata().map_err(|e|e.to_string())?.file_type().is_symlink(){
        return Err("Studio folder must not be a symlink".into());
    }
    let result=parent.join(id);
    if result.exists() && result.symlink_metadata().map_err(|e|e.to_string())?.file_type().is_symlink(){
        return Err("Studio project must not be a symlink".into());
    }
    Ok(result)
}
fn manifest(root:&Path,id:&str)->Result<PathBuf,String>{
    let path=dir(root,id)?.join("project.json");
    if path.exists() && path.symlink_metadata().map_err(|e|e.to_string())?.file_type().is_symlink() {
        return Err("Linked studio manifests are prohibited".into());
    }
    Ok(path)
}
fn child_dir(root:&Path,id:&str,child:&str)->Result<PathBuf,String>{
    if !matches!(child,"assets"|"renders"){return Err("Invalid studio directory".into())}
    let parent=dir(root,id)?;
    let result=parent.join(child);
    if result.exists() && result.symlink_metadata().map_err(|e|e.to_string())?.file_type().is_symlink(){
        return Err("Linked studio asset directories are prohibited".into());
    }
    std::fs::create_dir_all(&result).map_err(|e|e.to_string())?;
    let p=result.canonicalize().map_err(|e|e.to_string())?;
    if !p.starts_with(parent.canonicalize().map_err(|e|e.to_string())?){
        return Err("Studio output must remain inside its project".into());
    }
    Ok(result)
}
pub fn load(root:&Path,id:&str)->Result<StudioProject,String>{
    let path=manifest(root,id)?;
    let bytes=std::fs::read(path).map_err(|e|format!("Studio project not found: {e}"))?;
    if bytes.len()>400_000{return Err("Studio manifest exceeds limit".into());}
    serde_json::from_slice(&bytes).map_err(|e|format!("Invalid studio project data: {e}"))
}
fn persist(root:&Path,project:&mut StudioProject)->Result<(),String>{
    let folder=dir(root,&project.id)?;
    std::fs::create_dir_all(&folder).map_err(|e|e.to_string())?;
    project.updated_at=timestamp();
    let content=serde_json::to_vec_pretty(&project).map_err(|e|e.to_string())?;
    if content.len()>400_000{return Err("Studio manifest exceeds limit".into());}
    let temp=folder.join(format!("project.{}.tmp",Uuid::new_v4().simple()));
    std::fs::write(&temp,content).map_err(|e|e.to_string())?;
    let dest=folder.join("project.json");
    if let Err(e)=std::fs::rename(&temp,&dest){
        let _=std::fs::remove_file(&temp);
        return Err(format!("Cannot atomically save Studio project: {e}"));
    }
    Ok(())
}
pub fn create(root:&Path,args:&Map<String,Value>)->Result<String,String>{
    let name=str_arg(args,"name").trim();
    if name.is_empty() || name.len()>150 {return Err("Project name must be 1-150 characters".into());}
    let id=slug(if str_arg(args,"id").trim().is_empty(){name}else{str_arg(args,"id")})?;
    let dest=dir(root,&id)?;
    if dest.exists(){return Err(format!("Studio project '{id}' already exists; use studio_status instead of overwriting"))}
    let count=number(args,"scene_count",1) as usize;
    let duration=number(args,"scene_duration",10);
    if count==0 || count>MAX_SCENES{return Err("Scene count must be 1-24".into());}
    if duration==0 || duration>60{return Err("Each scene duration must be 1-60 seconds".into());}
    let style=str_arg(args,"style").trim();
    let aspect=match str_arg(args,"aspect"){""|"16:9"=>"16:9","9:16"=>"9:16","1:1"=>"1:1",_=>return Err("Aspect must be 16:9, 9:16, or 1:1".into())};
    let prompts=args.get("scene_prompts").and_then(Value::as_array).cloned().unwrap_or_default();
    if prompts.len()>count{return Err("More scene prompts than scene count".into());}
    let scenes=(0..count).map(|i|Scene{
        number:i as u32+1,duration,prompt:prompts.get(i).and_then(Value::as_str).unwrap_or("").to_string(),
        dialogue:String::new(),character_ids:vec![],voice_id:None,reference_image:None,
        image_task:None,image_asset:None,video_task:None,video_asset:None,error:None
    }).collect();
    let mut project=StudioProject{
        id:id.clone(),name:name.into(),style:style.into(),aspect:aspect.into(),created_at:timestamp(),
        updated_at:timestamp(),characters:vec![],voices:vec![],scenes,final_video:None,status:"planned".into()
    };
    persist(root,&mut project)?;
    Ok(format!("studio_project_created: {id}\nfolder: {}\nscenes: {count}\ntarget_duration_seconds: {}\nNo media has been rendered.",dest.display(),count as u32*duration))
}
pub fn status(root:&Path,id:&str)->Result<String,String>{
    let project=load(root,id)?;
    let scenes=project.scenes.iter().map(|s|json!({
        "scene":s.number,"duration":s.duration,"prompt_ready":!s.prompt.trim().is_empty(),
        "image_task":s.image_task,"image_asset":s.image_asset,
        "video_task":s.video_task,"video_asset":s.video_asset,"error":s.error
    })).collect::<Vec<_>>();
    Ok(json!({"project":project.id,"title":project.name,"status":project.status,
        "folder":dir(root,id)?.to_string_lossy(),"target_seconds":project.scenes.iter().map(|s|s.duration).sum::<u32>(),
        "characters":project.characters,"voices":project.voices,"scenes":scenes,
        "final_video":project.final_video}).to_string())
}
fn in_project_asset(root:&Path,id:&str,rel:&str)->Result<Option<String>,String>{
    if rel.trim().is_empty(){return Ok(None)}
    if !rel.starts_with("assets/"){return Err("Reference must be an imported assets/ filename".into());}
    let tail=rel.strip_prefix("assets/").unwrap();
    if !safe_token(tail) {return Err("Invalid asset reference".into());}
    let path=dir(root,id)?.join(rel);
    if !path.is_file() || path.symlink_metadata().map_err(|e|e.to_string())?.file_type().is_symlink(){
        return Err("Referenced project asset not found or is linked".into());
    }
    if !path.canonicalize().map_err(|e|e.to_string())?
        .starts_with(dir(root,id)?.canonicalize().map_err(|e|e.to_string())?) {
        return Err("Asset resolves outside this project".into());
    }
    Ok(Some(rel.into()))
}
pub fn upsert_character(root:&Path,args:&Map<String,Value>)->Result<String,String>{
    let id=str_arg(args,"project");
    let mut project=load(root,id)?;
    let character=slug(str_arg(args,"name"))?;
    let desc=str_arg(args,"description").trim();
    if desc.is_empty() || desc.len()>4000{return Err("Character description must be 1-4000 characters".into());}
    let image=in_project_asset(root,id,str_arg(args,"reference_image"))?;
    project.characters.retain(|c|c.id!=character);
    project.characters.push(Character{id:character.clone(),description:desc.into(),reference_image:image});
    persist(root,&mut project)?;
    Ok(format!("studio_character_saved: {character}\nproject: {id}\nCharacter is reusable across this project's scenes."))
}
pub fn upsert_voice(root:&Path,args:&Map<String,Value>)->Result<String,String>{
    let id=str_arg(args,"project");
    let mut project=load(root,id)?;
    let voice=slug(str_arg(args,"name"))?;
    let desc=str_arg(args,"description").trim();
    if desc.is_empty() || desc.len()>2000{return Err("Voice description must be 1-2000 characters".into())}
    let audio=in_project_asset(root,id,str_arg(args,"reference_audio"))?;
    let consent=args.get("permission_confirmed").and_then(Value::as_bool).unwrap_or(false);
    if audio.is_some() && !consent{return Err("Voice sample needs explicit permission_confirmed=true from the rights holder".into());}
    project.voices.retain(|v|v.id!=voice);
    project.voices.push(Voice{id:voice.clone(),description:desc.into(),reference_audio:audio,permission_confirmed:consent});
    persist(root,&mut project)?;
    Ok(format!("studio_voice_profile_saved: {voice}\nproject: {id}\nThis stores a voice identity/reference; it does not synthesize or clone a voice."))
}
pub fn import_asset(root:&Path,args:&Map<String,Value>)->Result<String,String>{
    let id=str_arg(args,"project");
    let _project=load(root,id)?;
    let source=str_arg(args,"source_path");
    if source.is_empty(){return Err("Missing workspace-relative source_path".into());}
    let relative=Path::new(source);
    if relative.is_absolute() || relative.components().any(|c|!matches!(c,std::path::Component::Normal(_))){return Err("Only simple workspace-relative paths allowed".into());}
    let canonical_root=std::fs::canonicalize(root).map_err(|e|e.to_string())?;
    let file=std::fs::canonicalize(root.join(relative)).map_err(|e|e.to_string())?;
    if !file.starts_with(&canonical_root)||!file.is_file(){return Err("Asset must be a regular file inside the workspace".into());}
    if file.symlink_metadata().map_err(|e|e.to_string())?.file_type().is_symlink(){return Err("Linked assets are not allowed".into());}
    let ext=file.extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !["png","jpg","jpeg","webp","wav","mp3","m4a","flac","mp4","webm"].contains(&ext.as_str()){return Err("Asset extension not supported".into());}
    let bytes=std::fs::metadata(&file).map_err(|e|e.to_string())?.len() as usize;
    if bytes==0 || bytes>MAX_ASSET_BYTES {return Err("Asset must be 1 byte to 32 MiB".into());}
    let name=slug(str_arg(args,"name"))?;
    let folder=child_dir(root,id,"assets")?;
    let dest=folder.join(format!("{name}.{ext}"));
    if dest.exists(){return Err("Project asset already exists; choose a different name".into());}
    std::fs::copy(&file,&dest).map_err(|e|e.to_string())?;
    Ok(format!("studio_asset_imported: assets/{name}.{ext}\nproject: {id}\nbytes: {bytes}"))
}
pub fn scene_update(root:&Path,args:&Map<String,Value>)->Result<String,String>{
    let id=str_arg(args,"project");
    let mut project=load(root,id)?;
    let ix=number(args,"scene",0);
    if ix==0 || ix as usize>project.scenes.len(){return Err("Scene index outside project".into());}
    let prompt=str_arg(args,"prompt").trim();
    if prompt.is_empty() || prompt.len()>4500{return Err("Scene prompt must be 1-4500 characters".into());}
    let characters=args.get("characters").and_then(Value::as_array).map(|a|a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default();
    if characters.iter().any(|c|!project.characters.iter().any(|p|&p.id==c)){return Err("A scene references an unknown character".into());}
    let voice=str_arg(args,"voice");
    if !voice.is_empty() && !project.voices.iter().any(|v|v.id==voice){return Err("A scene references an unknown voice profile".into());}
    let frame=in_project_asset(root,id,str_arg(args,"reference_image"))?;
    let dialogue=str_arg(args,"dialogue");
    if dialogue.len()>4000{return Err("Dialogue exceeds 4000 characters".into());}
    let scene=&mut project.scenes[ix as usize-1];
    scene.prompt=prompt.into();
    scene.dialogue=dialogue.into();
    scene.character_ids=characters;
    scene.voice_id=if voice.is_empty(){None}else{Some(voice.into())};
    scene.reference_image=frame;
    scene.video_task=None;
    scene.video_asset=None;
    scene.image_task=None;
    scene.image_asset=None;
    scene.error=None;
    project.final_video=None;
    project.status="planned".into();
    persist(root,&mut project)?;
    Ok(format!("studio_scene_saved: {ix}\nproject: {id}\nReady for asynchronous media generation."))
}
fn compiled_prompt(project:&StudioProject, scene:&Scene)->String{
    let mut text=format!("PROJECT: {}\nSTYLE: {}\nSCENE {} of {}, duration {} seconds.\n{}",
        project.name,project.style,scene.number,project.scenes.len(),scene.duration,scene.prompt);
    for name in &scene.character_ids {
        if let Some(ch)=project.characters.iter().find(|c|&c.id==name){
            text.push_str(&format!("\nCHARACTER {name}, same appearance as previous scenes: {}",ch.description));
        }
    }
    if let Some(id)=&scene.voice_id {
        if let Some(v)=project.voices.iter().find(|v|&v.id==id){
            text.push_str(&format!("\nVOICE CHARACTER {id}: {}. Use this vocal identity consistently if supported.",v.description));
        }
    }
    if !scene.dialogue.is_empty(){text.push_str(&format!("\nDIALOGUE: {}",scene.dialogue));}
    text.push_str("\nKeep the same characters, costumes and visual continuity across cuts.");
    text
}
fn load_asset_as_data(root:&Path,id:&str,rel:&str)->Result<String,String>{
    let allowed=if rel.starts_with("assets/"){
        in_project_asset(root,id,rel)?.ok_or("Frame asset missing")?
    }else if let Some(tail)=rel.strip_prefix("renders/"){
        if !safe_token(tail) {return Err("Invalid rendered image path".into())}
        let file=dir(root,id)?.join(rel);
        if !file.is_file(){return Err("Rendered reference image missing".into())}
        rel.to_string()
    }else{return Err("Frame must be a project image asset".into())};
    let file=dir(root,id)?.join(allowed);
    if !file.canonicalize().map_err(|e|e.to_string())?
        .starts_with(dir(root,id)?.canonicalize().map_err(|e|e.to_string())?){
        return Err("Frame path resolves outside project".into());
    }
    let ext=file.extension().and_then(|e|e.to_str()).unwrap_or("").to_lowercase();
    let mime=match ext.as_str(){"png"=>"image/png","jpeg"|"jpg"=>"image/jpeg","webp"=>"image/webp",_=>return Err("Frame asset must be PNG/JPEG/WebP".into())};
    let bytes=std::fs::read(file).map_err(|e|e.to_string())?;
    if bytes.len()>4*1024*1024{return Err("Frame image exceeds 4 MiB input limit".into())}
    use base64::Engine;
    Ok(format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes)))
}
fn media_extension(bytes:&[u8],kind:&str)->Result<&'static str,String>{
    if kind=="video" {
        if bytes.len()>12 && &bytes[4..8]==b"ftyp"{return Ok("mp4")}
        if bytes.starts_with(&[0x1A,0x45,0xDF,0xA3]){return Ok("webm")}
    } else {
        if bytes.starts_with(&[137,80,78,71,13,10,26,10]){return Ok("png")}
        if bytes.starts_with(&[0xFF,0xD8,0xFF]){return Ok("jpg")}
        if bytes.len()>12 && &bytes[0..4]==b"RIFF" && &bytes[8..12]==b"WEBP"{return Ok("webp")}
    }
    Err("Media result is not a recognized actual image/video file; refused to save fake output".into())
}
async fn save_media(root:&Path,id:&str,kind:&str,scene:u32,value:&Value,gw:&MediaGateway)->Result<String,String>{
    let url=if kind=="video"{value.pointer("/result/url").and_then(Value::as_str)}
        else{value.pointer("/result/data/0/url").and_then(Value::as_str)}
        .ok_or("Media task succeeded without a downloadable URL")?;
    let (bytes,_mime)=gw.asset(url).await?;
    let ext=media_extension(&bytes,kind)?;
    let folder=child_dir(root,id,"renders")?;
    let name=format!("scene_{scene:03}_{kind}.{ext}");
    std::fs::write(folder.join(&name),bytes).map_err(|e|e.to_string())?;
    Ok(format!("renders/{name}"))
}
pub async fn image_start(root:&Path,gw:&MediaGateway,args:&Map<String,Value>)->Result<String,String>{
    let id=str_arg(args,"project");
    let index=number(args,"scene",1);
    let mut project=load(root,id)?;
    if index==0||index as usize>project.scenes.len(){return Err("Invalid scene index".into())}
    let scene=project.scenes[index as usize-1].clone();
    let override_prompt=str_arg(args,"prompt");
    let prompt=if override_prompt.is_empty(){compiled_prompt(&project,&scene)}else{override_prompt.into()};
    if prompt.len()>14_000{return Err("Image prompt too large".into())}
    let frame=scene.reference_image.as_deref().map(|r|load_asset_as_data(root,id,r)).transpose()?;
    let task=gw.image(&prompt,&project.aspect,frame.as_deref()).await?;
    project.scenes[index as usize-1].image_task=Some(task.clone());
    project.status="image_queued".into();
    persist(root,&mut project)?;
    Ok(format!("studio_image_queued: {task}\nproject: {id}\nscene: {index}\nUse studio_status to check progress; image is not ready yet."))
}
async fn poll_task(root:&Path,id:&str,index:usize,kind:&str,gw:&MediaGateway)->Result<bool,String>{
    let project=load(root,id)?;
    let scene=&project.scenes[index];
    let task=if kind=="video"{scene.video_task.as_ref()}else{scene.image_task.as_ref()};
    let Some(task)=task else {return Ok(false)};
    let endpoint=if kind=="video"{"videos"}else{"images/generations"};
    let result=gw.task(endpoint,task).await?;
    let state=result.get("status").and_then(Value::as_str).unwrap_or("unknown");
    if matches!(state,"completed"|"succeeded"|"success") {
        let asset=save_media(root,id,kind,scene.number,&result,gw).await?;
        let mut project=load(root,id)?;
        let scene=&mut project.scenes[index];
        if kind=="video"{scene.video_asset=Some(asset.clone())} else{scene.image_asset=Some(asset.clone())}
        scene.error=None;
        project.status=format!("{kind}_complete");
        persist(root,&mut project)?;
        return Ok(true);
    }
    if matches!(state,"failed"|"cancelled"|"canceled"|"expired"){
        let err=result.get("error").map(|v|v.to_string()).unwrap_or_else(||state.into());
        let mut project=load(root,id)?;
        project.scenes[index].error=Some(err.clone());
        project.status=format!("{kind}_failed");
        persist(root,&mut project)?;
        return Err(format!("Scene {} {kind} failed: {}",index+1,err.chars().take(300).collect::<String>()));
    }
    Ok(false)
}
pub async fn refresh(root:&Path,id:&str,gw:&MediaGateway)->Result<String,String>{
    let project=load(root,id)?;
    for i in 0..project.scenes.len(){
        let scene=&project.scenes[i];
        if scene.video_task.is_some() && scene.video_asset.is_none(){let _=poll_task(root,id,i,"video",gw).await?;}
        if scene.image_task.is_some() && scene.image_asset.is_none(){let _=poll_task(root,id,i,"image",gw).await?;}
    }
    status(root,id)
}
fn runnable(project:&StudioProject)->Result<(),String>{
    for scene in &project.scenes{
        if scene.prompt.trim().is_empty(){return Err(format!("Scene {} has no storyboard prompt; use studio_set_scene",scene.number));}
    }
    Ok(())
}
struct ActiveGuard(String);
impl Drop for ActiveGuard{
    fn drop(&mut self){if let Ok(mut a)=ACTIVE.lock(){a.remove(&self.0);}}
}
pub fn start(root:PathBuf,id:String,gw:MediaGateway)->Result<String,String>{
    let project=load(&root,&id)?;
    runnable(&project)?;
    let key=format!("{}::{id}",root.display());
    let mut active=ACTIVE.lock().map_err(|_|"Studio lock poisoned")?;
    if !active.insert(key.clone()){return Err("Studio render already running for this project".into());}
    drop(active);
    let task_id=id.clone();
    tokio::spawn(async move{
        let id=task_id;
        let _guard=ActiveGuard(key);
        if let Err(err)=render_all(&root,&id,&gw).await {
            if let Ok(mut project)=load(&root,&id){
                project.status=format!("stopped: {}",err.chars().take(250).collect::<String>());
                let _=persist(&root,&mut project);
            }
        }
    });
    Ok(format!("studio_render_started: {id}\nVideo scenes render sequentially in the gateway background.\nThis is NOT a success receipt for MP4. Use studio_status or studio_refresh to monitor."))
}
async fn render_all(root:&Path,id:&str,gw:&MediaGateway)->Result<(),String>{
    let count=load(root,id)?.scenes.len();
    for i in 0..count{
        let project=load(root,id)?;
        let scene=project.scenes[i].clone();
        if scene.video_asset.is_some(){continue;}
        let task=if let Some(task)=scene.video_task {
            task
        }else{
            let prompt=compiled_prompt(&project,&scene);
            if prompt.len()>16_000{return Err(format!("Scene {} prompt too long",i+1));}
            let frame=scene.reference_image.as_deref().or(scene.image_asset.as_deref())
                .map(|r|load_asset_as_data(root,id,r)).transpose()?;
            let task=gw.video(&prompt,scene.duration,&project.aspect,frame.as_deref()).await?;
            let mut changed=load(root,id)?;
            changed.scenes[i].video_task=Some(task.clone());
            changed.status=format!("rendering_scene_{}",i+1);
            persist(root,&mut changed)?;
            task
        };
        let started=std::time::Instant::now();
        loop {
            if poll_task(root,id,i,"video",gw).await?{break;}
            if started.elapsed()>Duration::from_secs(15*60){
                return Err(format!("Scene {} timed out after 15 minutes; task {task} saved for retry",i+1));
            }
            sleep(Duration::from_secs(9)).await;
        }
    }
    let mut complete=load(root,id)?;
    complete.status="clips_ready".into();
    persist(root,&mut complete)?;
    // Automatically compose when FFmpeg is present. A composition error
    // leaves the individually verified scene clips untouched for retry.
    if ffmpeg().is_some() {
        compose(root,id).await?;
    }
    Ok(())
}
fn ffmpeg()->Option<PathBuf>{
    for base in [
        PathBuf::from("ffmpeg"),
        PathBuf::from("C:\\Program Files\\ffmpeg\\bin\\ffmpeg.exe"),
        PathBuf::from("C:\\muse-tauri\\tools\\ffmpeg\\bin\\ffmpeg.exe")
    ] {
        if base.is_absolute() && base.is_file(){return Some(base)}
        if !base.is_absolute(){
            if std::process::Command::new(&base).arg("-version")
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .status().map(|s|s.success()).unwrap_or(false){return Some(base)}
        }
    }
    None
}
pub async fn compose(root:&Path,id:&str)->Result<String,String>{
    let mut project=load(root,id)?;
    if project.scenes.iter().any(|s|s.video_asset.is_none()){return Err("Not all scenes have actual video files. Complete rendering first.".into());}
    let program=ffmpeg().ok_or("FFmpeg is not installed. Scene clips are safely stored; install a trusted FFmpeg build to enable final MP4 composition.")?;
    let folder=dir(root,id)?;
    let listing=folder.join("concat.txt");
    let mut text=String::new();
    for scene in &project.scenes{
        let rel=scene.video_asset.as_ref().unwrap();
        let tail=rel.strip_prefix("renders/").ok_or("Invalid rendered scene path")?;
        if !safe_token(tail){return Err("Invalid render filename".into());}
        let p=folder.join(rel);
        if !p.is_file(){return Err(format!("Missing scene file {rel}"))}
        let absolute=p.canonicalize().map_err(|e|e.to_string())?;
        if !absolute.starts_with(folder.canonicalize().map_err(|e|e.to_string())?){return Err("Scene file escapes project".into())}
        // concat demuxer uses quoted escaped absolute paths, never shell.
        text.push_str(&format!("file '{}'\n", absolute.to_string_lossy().replace('\\',"/").replace('\'',"'\\''")));
    }
    std::fs::write(&listing,text).map_err(|e|e.to_string())?;
    let output=folder.join("final.mp4");
    let status=Command::new(program).args(["-hide_banner","-loglevel","error","-y","-f","concat","-safe","0","-i"])
        .arg(&listing)
        .args(["-r","30","-c:v","libx264","-pix_fmt","yuv420p","-c:a","aac","-movflags","+faststart"])
        .arg(&output)
        .output().await.map_err(|e|format!("FFmpeg process failed: {e}"))?;
    if !status.status.success(){
        let _=std::fs::remove_file(&output);
        return Err(format!("FFmpeg cannot join scenes: {}",String::from_utf8_lossy(&status.stderr).chars().take(800).collect::<String>()));
    }
    let bytes=std::fs::metadata(&output).map_err(|e|e.to_string())?.len();
    use std::io::Read;
    let mut first=[0u8;16];
    std::fs::File::open(&output).map_err(|e|e.to_string())?
        .read_exact(&mut first).map_err(|e|e.to_string())?;
    media_extension(&first,"video")?;
    project.final_video=Some("final.mp4".into());
    project.status="complete".into();
    persist(root,&mut project)?;
    Ok(format!("studio_final_video_ready: {}\nbytes: {}\nscene_count: {}\nrequested_duration_seconds: {}",
        output.display(),bytes,project.scenes.len(),project.scenes.iter().map(|s|s.duration).sum::<u32>()))
}
pub async fn dispatch(root:&Path,gw:Option<&MediaGateway>,name:&str,args:&Map<String,Value>)->Result<String,String>{
    let id=str_arg(args,"project");
    match name {
        "studio_create" =>create(root,args),
        "studio_status" =>status(root,id),
        "studio_character" =>upsert_character(root,args),
        "studio_voice" =>upsert_voice(root,args),
        "studio_import_asset" =>import_asset(root,args),
        "studio_set_scene" =>scene_update(root,args),
        "studio_render_image" =>image_start(root,gw.ok_or("Media gateway unavailable")?,args).await,
        "studio_refresh" =>refresh(root,id,gw.ok_or("Media gateway unavailable")?).await,
        "studio_render_project" =>start(root.to_path_buf(),id.into(),gw.ok_or("Media gateway unavailable")?.clone()),
        "studio_compose" =>compose(root,id).await,
        _=>Err("Unknown Muse Studio tool".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn test_root()->PathBuf{
        let dir=std::env::temp_dir().join(format!("muse-studio-{}",Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn project_is_durable_and_uses_proper_scene_duration(){
        let root=test_root();
        let map=serde_json::from_value::<Map<String,Value>>(json!({
            "name":"Andi Maya","scene_count":6,"scene_duration":10,
            "scene_prompts":["forest","mountain","cave","ocean","city","sunrise"]
        })).unwrap();
        let receipt=create(&root,&map).unwrap();
        assert!(receipt.contains("target_duration_seconds: 60"));
        let mut p=load(&root,"andi-maya").unwrap();
        assert_eq!(p.scenes.len(),6);
        assert_eq!(p.scenes.iter().map(|s|s.duration).sum::<u32>(),60);
        assert!(create(&root,&map).is_err());
        p.scenes[0].dialogue="Hello".into();
        persist(&root,&mut p).unwrap();
        assert_eq!(load(&root,"andi-maya").unwrap().scenes[0].dialogue,"Hello");
        let _=std::fs::remove_dir_all(root);
    }
    #[test]
    fn voice_requires_permission_and_assets_cannot_escape_workspace(){
        let root=test_root();
        let args=serde_json::from_value::<Map<String,Value>>(json!({"name":"test"})).unwrap();
        create(&root,&args).unwrap();
        std::fs::write(root.join("record.wav"),"RIFFabcdef").unwrap();
        let imp=serde_json::from_value::<Map<String,Value>>(json!({"project":"test","source_path":"record.wav","name":"voice1"})).unwrap();
        assert!(import_asset(&root,&imp).unwrap().contains("assets/voice1.wav"));
        let voice=serde_json::from_value::<Map<String,Value>>(json!({"project":"test","name":"narrator","description":"Warm female voice","reference_audio":"assets/voice1.wav"})).unwrap();
        assert!(upsert_voice(&root,&voice).is_err());
        let mut valid=voice;
        valid.insert("permission_confirmed".into(),json!(true));
        assert!(upsert_voice(&root,&valid).is_ok());
        let bad=serde_json::from_value::<Map<String,Value>>(json!({"project":"test","source_path":"../secret.wav","name":"bad"})).unwrap();
        assert!(import_asset(&root,&bad).is_err());
        let _=std::fs::remove_dir_all(root);
    }
    #[test]
    fn media_path_accepts_gateway_relative_url_without_untrusted_network(){
        let file="vid_933b10307633a987.mp4";
        assert_eq!(media_result_name(&format!("/v1/media/{file}")).unwrap(),file);
        assert_eq!(media_result_name(&format!("https://remote.example/v1/media/{file}")).unwrap(),file);
        for unsafe_url in [
            "https://example.invalid/assets/video.mp4",
            "/private/video.mp4",
            "/v1/media/../secret.mp4",
            "/v1/media/video.mp4?token=x",
            "/v1/media/%2e%2e%2fsecret.mp4",
            "file:///v1/media/video.mp4",
            "ftp://example.invalid/v1/media/video.mp4",
            "https://remote.example/v1/media/video.mp4?access=secret",
        ]{
            assert!(media_result_name(unsafe_url).is_err(),"URL must be rejected: {unsafe_url}");
        }
    }
    #[test]
    fn corrupted_media_is_not_treated_as_success(){
        assert!(media_extension(b"<html>download failed</html>","video").is_err());
        assert_eq!(media_extension(b"\0\0\0\x18ftypmp42xxxx","video").unwrap(),"mp4");
        assert!(media_extension(b"not an image","image").is_err());
    }
}
