use std::{net::SocketAddr, path::PathBuf};
use serde_json::{json,Map,Value};
use muse_agent::{studio::{self,MediaGateway},executor::ToolExecutor,parser::ToolCall,error::CancellationToken,intent::{self,work},approval};
use tokio::{net::{TcpListener,TcpStream},io::{AsyncReadExt,AsyncWriteExt}};
use uuid::Uuid;

fn workspace()->PathBuf{
    let root=std::env::temp_dir().join(format!("muse-studio-qa-{}",Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn args(v:Value)->Map<String,Value>{v.as_object().unwrap().clone()}
fn call(name:&str,v:Value)->ToolCall{ToolCall::new(name,args(v))}

#[tokio::test]
async fn studio_tools_create_and_reuse_character_voice_across_scenes(){
    let root=workspace();
    let tools=ToolExecutor::new(&root,"auto").unwrap();
    let cancel=CancellationToken::new();
    let create=call("studio_create",json!({
        "name":"Andi dan Maya","scene_count":6,"scene_duration":10,"style":"3D cinematic",
        "scene_prompts":["forest","river","mountain","temple","night","sunrise"]
    }));
    assert!(ToolExecutor::needs_confirmation(&create));
    let out=tools.execute(&create,&cancel,None).await.unwrap();
    assert!(out.contains("target_duration_seconds: 60"),"{out}");
    let ch=call("studio_character",json!({"project":"andi-dan-maya","name":"Andi","description":"3D adventurer, blue jacket, round glasses"}));
    let out=tools.execute(&ch,&cancel,None).await.unwrap();
    assert!(out.starts_with("studio_character_saved"),"{out}");
    let voice=call("studio_voice",json!({"project":"andi-dan-maya","name":"narrator","description":"Warm synthetic adult voice"}));
    assert!(tools.execute(&voice,&cancel,None).await.unwrap().starts_with("studio_voice_profile_saved"));
    let scene=call("studio_set_scene",json!({
        "project":"andi-dan-maya","scene":1,"prompt":"Andi arrives in a forest",
        "dialogue":"Halo, Maya!","characters":["andi"],"voice":"narrator"
    }));
    assert!(tools.execute(&scene,&cancel,None).await.unwrap().starts_with("studio_scene_saved"));
    let project=studio::load(&root,"andi-dan-maya").unwrap();
    assert_eq!(project.characters.len(),1);
    assert_eq!(project.voices.len(),1);
    assert_eq!(project.scenes[0].character_ids,vec!["andi"]);
    assert_eq!(project.scenes[0].voice_id.as_deref(),Some("narrator"));
    let status=tools.execute(&call("studio_status",json!({"project":"andi-dan-maya"})),&cancel,None).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&status).unwrap()["target_seconds"],60);
    assert!(studio::load(&root,"../bad").is_err());
    let _=std::fs::remove_dir_all(root);
}

#[test]
fn media_render_never_autoruns_without_explicit_approval(){
    for name in ["studio_render_project","studio_render_image","studio_compose"]{
        let call=call(name,json!({"project":"demo"}));
        assert!(ToolExecutor::needs_confirmation(&call));
        assert!(approval::requires_explicit_approval(&call));
    }
    assert_eq!(intent::required("buatkan video 60 detik 6 scene") & work::STUDIO_RENDER,work::STUDIO_RENDER);
    assert_eq!(intent::required("buatkan gambar karakter 3D") & work::STUDIO_RENDER,work::STUDIO_RENDER);
    assert_eq!(intent::required("buatkan storyboard karakter") & work::STUDIO_PLAN,work::STUDIO_PLAN);
    assert_eq!(intent::required("apakah bisa buat video?") & work::STUDIO_RENDER,0);
    assert!(!intent::allows("hello","studio_render_project",false));
    assert!(intent::allows("buatkan video karakter 3D","studio_render_project",false));
    assert!(intent::allows("buatkan video karakter 3D","studio_create",false));
}

async fn serve(mut stream:TcpStream){
    let mut bytes=Vec::new();let mut temp=[0u8;4096];
    loop{
        let n=stream.read(&mut temp).await.unwrap_or(0);
        if n==0{break;}
        bytes.extend_from_slice(&temp[..n]);
        if let Some(pos)=bytes.windows(4).position(|v|v==b"\r\n\r\n"){
            let head=String::from_utf8_lossy(&bytes[..pos]);
            let length=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v|v.trim().parse::<usize>().ok())).unwrap_or(0);
            if bytes.len()>=pos+4+length{break;}
        }
        if bytes.len()>200_000{return;}
    }
    let request=String::from_utf8_lossy(&bytes);
    let first=request.lines().next().unwrap_or("");
    let (content,mime)=if first.starts_with("POST /v1/videos "){
        (br#"{"id":"vid_abc","status":"queued"}"#.to_vec(),"application/json")
    }else if first.starts_with("GET /v1/videos/vid_abc "){
        (br#"{"id":"vid_abc","status":"completed","result":{"url":"/v1/media/scene_a.mp4"}}"#.to_vec(),"application/json")
    }else if first.starts_with("GET /v1/media/scene_a.mp4 "){
        (b"\0\0\0\x18ftypisom0000fakepayload".to_vec(),"video/mp4")
    }else{
        (br#"{"error":"not_found"}"#.to_vec(),"application/json")
    };
    let response=format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",content.len());
    let _=stream.write_all(response.as_bytes()).await;
    let _=stream.write_all(&content).await;
}
#[tokio::test]
async fn media_gateway_queues_and_downloads_only_from_localhost(){
    let root=workspace();
    studio::create(&root,&args(json!({"name":"Video Test","scene_count":1,"scene_prompts":["sunrise on mountain"]}))).unwrap();
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address:SocketAddr=listener.local_addr().unwrap();
    let server=tokio::spawn(async move{
        for _ in 0..3 { let (stream,_)=listener.accept().await.unwrap();serve(stream).await; }
    });
    let gateway=MediaGateway::new(address.port(),"test-key".into()).unwrap();
    let task=gateway.video("sunrise",10,"16:9",None).await.unwrap();
    assert_eq!(task,"vid_abc");
    // Record the exact returned task (the production render worker does so
    // immediately before polling). The gateway returns a relative URL;
    // MediaGateway still fetches only from its local media store.
    let meta=root.join(".muse-studio/video-test/project.json");
    let mut project:Value=serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    project["scenes"][0]["video_task"]=json!(task);
    std::fs::write(&meta,serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    let state=studio::refresh(&root,"video-test",&gateway).await.unwrap();
    assert!(state.contains("scene_001_video.mp4"),"{state}");
    let file=root.join(".muse-studio/video-test/renders/scene_001_video.mp4");
    assert!(file.is_file());
    assert_eq!(&std::fs::read(file).unwrap()[4..8],b"ftyp");
    server.await.unwrap();
    let _=std::fs::remove_dir_all(root);
}


#[tokio::test]
async fn studio_joins_six_actual_video_clips_into_one_mp4(){
    // The acceptance test runs only if a local trusted FFmpeg is present.
    let ff=PathBuf::from(r"C:\muse-tauri\tools\ffmpeg\bin\ffmpeg.exe");
    let probe=PathBuf::from(r"C:\muse-tauri\tools\ffmpeg\bin\ffprobe.exe");
    if !(ff.is_file() && probe.is_file()) {
        eprintln!("FFmpeg not installed on this test host; compositor smoke skipped");
        return;
    }
    let root=workspace();
    let id="six-real-clips";
    studio::create(&root,&args(json!({
        "name":"Six Real Clips",
        "scene_count":6,"scene_duration":1,"scene_prompts":["1","2","3","4","5","6"]
    }))).unwrap();
    let folder=root.join(".muse-studio").join(id);
    let renders=folder.join("renders");
    std::fs::create_dir_all(&renders).unwrap();
    let manifest_path=folder.join("project.json");
    let mut project:Value=serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    for i in 1..=6u32{
        let fname=format!("scene_{i:03}_video.mp4");
        let path=renders.join(&fname);
        let command=tokio::process::Command::new(&ff)
            .args(["-hide_banner","-loglevel","error","-y",
                "-f","lavfi","-i","color=c=blue:s=160x90:r=10:d=1",
                "-c:v","libx264","-pix_fmt","yuv420p"])
            .arg(&path)
            .output().await.expect("create a local dummy MP4");
        assert!(command.status.success(),"{}",String::from_utf8_lossy(&command.stderr));
        project["scenes"][i as usize-1]["video_asset"]=json!(format!("renders/{fname}"));
    }
    std::fs::write(&manifest_path,serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    let result=studio::compose(&root,id).await.expect("join six valid scene videos");
    assert!(result.contains("studio_final_video_ready"));
    let final_path=folder.join("final.mp4");
    assert!(final_path.is_file());
    let result=tokio::process::Command::new(&probe)
        .args(["-v","error","-show_entries","format=duration",
            "-of","default=noprint_wrappers=1:nokey=1"])
        .arg(&final_path).output().await.unwrap();
    assert!(result.status.success());
    let duration:String=String::from_utf8_lossy(&result.stdout).trim().into();
    let duration:f64=duration.parse().unwrap();
    assert!(duration>5.5 && duration<6.5,"final length is {duration}, expected six seconds");
    assert_eq!(studio::load(&root,id).unwrap().status,"complete");
    let _=std::fs::remove_dir_all(root);
}


#[test]
fn native_media_requests_cannot_fall_back_to_python_animation() {
    use muse_agent::intent::{self,work};
    for prompt in [
        "buatkan video masha and the beer 10 detik",
        "buatkan video Masha and the Bear 10 detik",
        "buatkan video karakter 3D enam scene masing masing 10 detik",
        "buatkan gambar karakter Masha 3D",
    ] {
        let required=intent::required(prompt);
        assert_eq!(required & work::STUDIO_RENDER,work::STUDIO_RENDER,"{prompt}: {required}");
        for tool in ["run_command","write_file","edit_file","process_input","computer_click","install_github_skill"] {
            assert!(!intent::allows(prompt,tool,false),"{prompt}: {tool} should be BLOCKED");
            assert!(!intent::allows(prompt,tool,true),"{prompt}: {tool} should be BLOCKED even in goal mode");
        }
        for tool in ["studio_create","studio_render_project","studio_render_image","studio_status","list_files"] {
            assert!(intent::allows(prompt,tool,false),"{prompt}: {tool} should be allowed");
        }
    }
    assert_eq!(intent::required("apakah bisa membuat video?") & work::STUDIO_RENDER,0);
    assert_eq!(intent::required("buatkan script Python untuk video 10 detik") & work::STUDIO_RENDER,0);
    assert!(intent::allows("buatkan script Python untuk video 10 detik","write_file",false));
    assert!(intent::allows("buatkan program Python untuk animasi video","run_command",false));
    assert_eq!(intent::required("buatkan hanya storyboard karakter") & work::STUDIO_PLAN,work::STUDIO_PLAN);
    assert!(!intent::allows("buatkan hanya storyboard karakter","studio_render_project",false));
}


#[test]
fn quick_media_route_handles_plain_ten_second_video_without_model() {
    let (args,tool)=studio::quick_plan("Buatkan video 3D Masha and the Bear 10 detik.").unwrap();
    assert_eq!(tool,"studio_render_project");
    assert_eq!(args["scene_count"],1);
    assert_eq!(args["scene_duration"],10);
    assert_eq!(args["style"],"3D");
    assert!(args["scene_prompts"][0].as_str().unwrap().contains("Masha and the Bear"));
    assert!(args["id"].as_str().unwrap().starts_with("muse-video-"));
    let (image,tool)=studio::quick_plan("buatkan gambar rumah di hutan").unwrap();
    assert_eq!(tool,"studio_render_image");
    assert_eq!(image["scene_count"],1);
    for should_use_planner in [
        "apakah bisa buat video 10 detik?",
        "buatkan script Python untuk video 10 detik",
        "buatkan video 3D 6 scene masing-masing 10 detik",
        "buatkan video dari folder karakter saya 10 detik",
        "cek status project video",
    ] {
        assert!(studio::quick_plan(should_use_planner).is_none(),
            "Should leave complex/script/question task to planner: {should_use_planner}");
    }
}
