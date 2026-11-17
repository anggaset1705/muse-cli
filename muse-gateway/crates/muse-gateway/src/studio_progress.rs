//! Read-only Studio render progress for a workspace-scoped AgentSession.
//! Never starts, retries, or cancels media generation. Percentages use the
//! gateway TaskStore's actual provider progress and verified local outputs.
use std::path::Path;

use muse_agent::studio::StudioProject;
use muse_store::TaskStore;
use serde_json::{json, Value};

fn stored_file(root: &Path, id: &str, relative: &str) -> Option<String> {
    // Project IDs were already validated by studio::load, but we do not
    // trust filenames inside user-editable project.json.
    let file = if relative == "final.mp4" {
        root.join(".muse-studio").join(id).join("final.mp4")
    } else if relative.starts_with("renders/") {
        let name = relative.strip_prefix("renders/")?;
        if name.is_empty()
            || name.len() > 100
            || name.contains("..")
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return None;
        }
        root.join(".muse-studio").join(id).join("renders").join(name)
    } else {
        return None;
    };
    let project_dir = root.join(".muse-studio").join(id).canonicalize().ok()?;
    let metadata = file.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let real = file.canonicalize().ok()?;
    if !real.starts_with(&project_dir) {
        return None;
    }
    Some(real.to_string_lossy().into_owned())
}

/// One snapshot only: neither requests external media nor modifies the
/// project. Clients poll it, at an interval, even after the chat turn ends.
pub fn snapshot(project: &StudioProject, root: &Path, tasks: &TaskStore) -> Value {
    let is_image = project.id.starts_with("muse-image-")
        || project.status.starts_with("image_");
    let mut completed: usize = 0;
    let mut active: Option<u32> = None;
    let mut sum_percent_weighted: u64 = 0;
    let mut total_weight: u64 = 0;
    let mut any_queued = false;
    let mut any_running = false;
    let mut any_saving = false;
    let mut any_missing_task = false;
    let mut failure: Option<String> = None;
    let mut scenes = Vec::with_capacity(project.scenes.len());

    for scene in &project.scenes {
        let file_ref = if is_image {
            scene.image_asset.as_deref()
        } else {
            scene.video_asset.as_deref()
        };
        let verified_file = file_ref.and_then(|rel| stored_file(root, &project.id, rel));
        let saved = verified_file.is_some();
        let task_id = if is_image {
            scene.image_task.as_deref()
        } else {
            scene.video_task.as_deref()
        };
        let task = task_id.and_then(|id| tasks.get(id));
        let weight = if is_image { 1u64 } else { u64::from(scene.duration.max(1)) };
        total_weight += weight;
        let percent = if saved {
            100
        } else if let Some(task) = &task {
            task.progress.clamp(0, 100)
        } else {
            0
        };
        if saved {
            completed += 1;
        } else if active.is_none() {
            active = Some(scene.number);
        }
        // The gateway may mark a provider task succeeded before Muse Studio
        // has copied its validated media into this project's renders folder.
        // That is "saving", not "completed".
        sum_percent_weighted += weight * u64::from(percent as u32);
        let state = if saved {
            "saved"
        } else if let Some(t) = &task {
            match t.status.as_str() {
                "queued" => {
                    any_queued = true;
                    "queued"
                }
                "running" => {
                    any_running = true;
                    "rendering"
                }
                "succeeded" => {
                    any_saving = true;
                    "saving"
                }
                "cancelled" | "failed" => {
                    let msg = t.error.as_ref()
                        .and_then(|e| e.get("message")).and_then(Value::as_str)
                        .unwrap_or("Video provider task failed or was interrupted");
                    failure.get_or_insert_with(|| msg.chars().take(240).collect());
                    "failed"
                }
                _ => {
                    any_missing_task = true;
                    "unknown"
                }
            }
        } else if task_id.is_some() {
            any_missing_task = true;
            "unknown"
        } else {
            any_queued = true;
            "waiting"
        };
        if let Some(msg) = &scene.error {
            failure.get_or_insert_with(|| msg.chars().take(240).collect());
        }
        scenes.push(json!({
            "number": scene.number,
            "duration": scene.duration,
            "state": state,
            "provider_progress": task.as_ref().map(|_| percent),
            "task_id": task_id,
            "saved": saved,
            "error": scene.error.as_ref().map(|s|s.chars().take(240).collect::<String>())
        }));
    }

    let final_file = project.final_video.as_deref()
        .and_then(|rel| stored_file(root, &project.id, rel));
    let project_finished = project.status == "complete" && final_file.is_some();
    if project.status.starts_with("stopped:") {
        failure.get_or_insert_with(|| project.status
            .strip_prefix("stopped:").unwrap_or("render stopped")
            .trim().chars().take(240).collect());
    }
    if project.status == "complete" && final_file.is_none() && !is_image {
        failure.get_or_insert_with(|| "Final MP4 is missing from this project".into());
    }
    let state = if project_finished {
        "complete"
    } else if failure.is_some() {
        "failed"
    } else if completed == project.scenes.len() && !project.scenes.is_empty() {
        if is_image { "complete" } else { "composing" }
    } else if any_saving {
        "saving"
    } else if any_running {
        "rendering"
    } else if any_missing_task {
        "unknown"
    } else if any_queued {
        "queued"
    } else {
        "waiting"
    };
    // Weighted by requested scene duration for multi-scene video. All
    // inputs are from the actual TaskStore; 100 means file is on disk.
    let raw = if total_weight > 0 {
        (sum_percent_weighted / total_weight) as u32
    } else {
        0
    };
    let progress = if project_finished || (is_image && state == "complete") {
        100
    } else {
        raw.min(99)
    };
    json!({
        "project_id":project.id,
        "name":project.name,
        "state":state,
        "progress":progress,
        "completed_scenes":completed,
        "scene_count":project.scenes.len(),
        "active_scene":if project_finished {None} else {active},
        "scenes":scenes,
        "final_video_path":final_file,
        "error":failure,
        "updated_at":project.updated_at
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};
    use uuid::Uuid;
    use std::fs;

    fn fixture() -> (std::path::PathBuf, StudioProject, TaskStore, String) {
        let root = std::env::temp_dir().join(format!("studio-progress-{}",Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let args: Map<String, Value> = serde_json::from_value(json!({
            "name":"Bear Fishing",
            "scene_count":2, "scene_duration":10,
            "scene_prompts":["Bear fishes","Bear smiles"]
        })).unwrap();
        muse_agent::studio::create(&root, &args).unwrap();
        let mut project = muse_agent::studio::load(&root,"bear-fishing").unwrap();
        let mut store=TaskStore::new(root.join("tasks.json"));
        let task = store.create("video", Map::new()).unwrap();
        project.scenes[0].video_task=Some(task.id.clone());
        (root,project,store,task.id)
    }

    #[test]
    fn actual_gateway_progress_is_reported_without_claiming_early_success() {
        let (root,mut project,mut store,id)=fixture();
        let queued=snapshot(&project,&root,&store);
        assert_eq!(queued["state"],"queued");
        assert_eq!(queued["progress"],0);
        store.set_running(&id).unwrap();
        store.set_progress(&id,60).unwrap();
        let running=snapshot(&project,&root,&store);
        assert_eq!(running["state"],"rendering");
        assert_eq!(running["scenes"][0]["provider_progress"],60);
        assert_eq!(running["progress"],30); // 60% of one of two 10s scenes
        store.set_succeeded(&id,Map::new()).unwrap();
        let saving=snapshot(&project,&root,&store);
        assert_eq!(saving["state"],"saving");
        assert_eq!(saving["progress"],50);
        let renders=root.join(".muse-studio/bear-fishing/renders");
        fs::create_dir_all(&renders).unwrap();
        fs::write(renders.join("scene_001_video.mp4"),b"real output placeholder").unwrap();
        project.scenes[0].video_asset=Some("renders/scene_001_video.mp4".into());
        let second=store.create("video",Map::new()).unwrap();
        project.scenes[1].video_task=Some(second.id.clone());
        store.set_running(&second.id).unwrap();
        store.set_progress(&second.id,40).unwrap();
        let mid=snapshot(&project,&root,&store);
        assert_eq!(mid["progress"],70);
        assert_eq!(mid["completed_scenes"],1);
        store.set_succeeded(&second.id,Map::new()).unwrap();
        fs::write(renders.join("scene_002_video.mp4"),b"verified output placeholder").unwrap();
        project.scenes[1].video_asset=Some("renders/scene_002_video.mp4".into());
        let composing=snapshot(&project,&root,&store);
        assert_eq!(composing["state"],"composing");
        assert_eq!(composing["progress"],99);
        let final_path=root.join(".muse-studio/bear-fishing/final.mp4");
        fs::write(&final_path,b"joined output placeholder").unwrap();
        project.status="complete".into();
        project.final_video=Some("final.mp4".into());
        let done=snapshot(&project,&root,&store);
        assert_eq!(done["state"],"complete");
        assert_eq!(done["progress"],100);
        assert!(done["final_video_path"].as_str().unwrap().ends_with("final.mp4"));
        let _=fs::remove_dir_all(root);
    }

    #[test]
    fn interruption_and_missing_files_never_claim_finished() {
        let (root,mut project,mut store,id)=fixture();
        store.set_failed(&id, "interrupted", "interrupted by server restart").unwrap();
        let progress=snapshot(&project,&root,&store);
        assert_eq!(progress["state"],"failed");
        assert_eq!(progress["progress"],0);
        assert!(progress["error"].as_str().unwrap().contains("interrupted"));
        project.status="complete".into();
        project.final_video=Some("../outside.mp4".into());
        let unsafe_file=snapshot(&project,&root,&store);
        assert!(unsafe_file["final_video_path"].is_null());
        assert_ne!(unsafe_file["state"],"complete");
        let _=fs::remove_dir_all(root);
    }
}
