//! Action policy — pure validation, no OS calls.
//!
//! Port of `ComputerActionPolicy` (`ComputerGeometry.cs`, .NET): which
//! actions mutate the UI (and therefore invalidate the screenshot
//! snapshot), the supported key chords (virtual-key codes), and the
//! argument validation from `Validate(ToolCall)`.

use crate::actions::{ScreenAction, ScreenArgs};
use crate::error::{ScreenError, ScreenResult};

/// Whether the action mutates the UI (`.NET`: `Mutates`). A mutation
/// invalidates the current screenshot snapshot.
pub fn mutates(action: &ScreenAction) -> bool {
    matches!(
        action,
        ScreenAction::Focus
            | ScreenAction::Invoke
            | ScreenAction::SetValue
            | ScreenAction::Type
            | ScreenAction::Key
            | ScreenAction::Toggle
            | ScreenAction::Select
            | ScreenAction::Expand
            | ScreenAction::Scroll
            | ScreenAction::Click
            | ScreenAction::Drag
    )
}

/// Translate a key chord (`.NET` `Keys()`) to virtual-key codes.
///
/// Supported chords are exactly the .NET table; anything else (including
/// app/system switching keys) is rejected. `Esc` is reserved for the user
/// to stop control and is never sent.
pub fn keys_for_chord(chord: &str) -> ScreenResult<Vec<u16>> {
    let keys: &[u16] = match chord.trim().to_uppercase().as_str() {
        "ENTER" => &[13],
        "TAB" => &[9],
        "SHIFT+TAB" => &[16, 9],
        "SPACE" => &[32],
        "CTRL+A" => &[17, 65],
        "CTRL+C" => &[17, 67],
        "CTRL+V" => &[17, 86],
        "CTRL+Z" => &[17, 90],
        "CTRL+S" => &[17, 83],
        "CTRL+F" => &[17, 70],
        "CTRL+L" => &[17, 76],
        "CTRL+HOME" => &[17, 36],
        "CTRL+END" => &[17, 35],
        "BACKSPACE" => &[8],
        "DELETE" => &[46],
        "LEFT" => &[37],
        "RIGHT" => &[39],
        "UP" => &[38],
        "DOWN" => &[40],
        "HOME" => &[36],
        "END" => &[35],
        "PAGEUP" => &[33],
        "PAGEDOWN" => &[34],
        "F5" => &[116],
        _ => {
            return Err(ScreenError::InvalidInput(
                "This key is not supported yet. Esc is reserved for you to stop control; do not use app/system switching keys."
                    .to_string(),
            ));
        }
    };
    Ok(keys.to_vec())
}

/// Validate an action + arguments, mirroring
/// `ComputerActionPolicy.Validate` (`.NET`). Runs before any OS call, on
/// every platform.
pub fn validate(action: &ScreenAction, args: &ScreenArgs) -> ScreenResult<()> {
    let element_id = args.element_id.as_deref().unwrap_or("");
    let invalid = |msg: &str| ScreenError::InvalidInput(msg.to_string());

    if matches!(
        action,
        ScreenAction::Invoke
            | ScreenAction::SetValue
            | ScreenAction::Assert
            | ScreenAction::Toggle
            | ScreenAction::Select
            | ScreenAction::Expand
    ) && element_id.trim().is_empty()
    {
        return Err(invalid(
            "An element_id from the latest UI read is required.",
        ));
    }

    if *action == ScreenAction::Inspect {
        let check_int = |name: &str, v: Option<i32>, min: i32, max: i32| -> ScreenResult<()> {
            if let Some(n) = v {
                if n < min || n > max {
                    return Err(invalid(&format!(
                        "{name} must be an integer from {min} to {max}."
                    )));
                }
            }
            Ok(())
        };
        check_int("offset", args.offset, 0, 10_000)?;
        check_int("limit", args.limit, 1, 180)?;
        check_int("max_depth", args.max_depth, 1, 30)?;
        if args.query.as_deref().unwrap_or("").len() > 200 {
            return Err(invalid(
                "The UI search keyword is too long (maximum 200 characters).",
            ));
        }
    }

    if *action == ScreenAction::Key {
        keys_for_chord(args.key.as_deref().unwrap_or(""))?;
    }

    if *action == ScreenAction::Click {
        match args.button.as_deref().unwrap_or("") {
            "" | "left" | "right" => {}
            _ => return Err(invalid("button must be left or right.")),
        }
    }

    if *action == ScreenAction::Scroll {
        match args.direction.as_deref().unwrap_or("") {
            "up" | "down" | "left" | "right" => {}
            _ => return Err(invalid("direction must be up/down/left/right.")),
        }
    }

    let needs_snapshot = matches!(action, ScreenAction::Click | ScreenAction::Drag)
        || (*action == ScreenAction::Scroll && element_id.is_empty());
    if needs_snapshot {
        if args.snapshot_id.as_deref().unwrap_or("").trim().is_empty() {
            return Err(invalid(
                "Take a new screenshot and provide a snapshot_id before using coordinates.",
            ));
        }
        let coords: &[Option<i32>] = if *action == ScreenAction::Drag {
            &[args.x, args.y, args.to_x, args.to_y]
        } else {
            &[args.x, args.y]
        };
        for (i, c) in coords.iter().enumerate() {
            let name = if *action == ScreenAction::Drag {
                ["x", "y", "to_x", "to_y"][i]
            } else {
                ["x", "y"][i]
            };
            match c {
                Some(v) if *v >= 0 => {}
                _ => {
                    return Err(invalid(&format!(
                        "{name} must be a non-negative integer pixel coordinate in the new image."
                    )));
                }
            }
        }
    }

    if matches!(action, ScreenAction::Type | ScreenAction::SetValue) {
        let text = match &args.text {
            Some(t) => t,
            None => {
                return Err(invalid(
                    "Provide text as a string; to clear an input, use empty text.",
                ));
            }
        };
        let limit = if *action == ScreenAction::Type {
            4000
        } else {
            16000
        };
        if text.len() > limit
            || text.contains('\0')
            || (*action == ScreenAction::Type && text.is_empty())
        {
            return Err(invalid(&format!(
                "Invalid input content (at most {limit} characters, no NUL)."
            )));
        }
    }

    if *action == ScreenAction::Assert {
        match args.property.as_deref().unwrap_or("") {
            "name" | "value" | "toggle" | "enabled" | "offscreen" | "selected" => {}
            _ => {
                return Err(invalid(
                    "This property is not supported for UI checks.",
                ));
            }
        }
        if args.expected.is_none() {
            return Err(invalid(
                "An expected value is required to verify the result; missing data cannot count as a passed test.",
            ));
        }
        match args.match_.as_deref().unwrap_or("") {
            "" | "equals" | "contains" => {}
            _ => return Err(invalid("match must be equals or contains.")),
        }
        if let Some(w) = args.wait_ms {
            if !(0..=5000).contains(&w) {
                return Err(invalid("wait_ms must be from 0 to 5000."));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{ScreenAction, ScreenArgs};

    #[test]
    fn key_table_matches_dotnet() {
        assert_eq!(keys_for_chord("enter").unwrap(), vec![13]);
        assert_eq!(keys_for_chord("CTRL+A").unwrap(), vec![17, 65]);
        assert_eq!(keys_for_chord("shift+tab").unwrap(), vec![16, 9]);
        assert_eq!(keys_for_chord("F5").unwrap(), vec![116]);
        assert!(keys_for_chord("ESC").is_err());
        assert!(keys_for_chord("ALT+TAB").is_err());
        assert!(keys_for_chord("").is_err());
    }

    #[test]
    fn mutates_matches_dotnet() {
        assert!(mutates(&ScreenAction::Click));
        assert!(mutates(&ScreenAction::Type));
        assert!(mutates(&ScreenAction::Scroll));
        assert!(!mutates(&ScreenAction::Inspect));
        assert!(!mutates(&ScreenAction::Screenshot));
        assert!(!mutates(&ScreenAction::Assert));
        assert!(!mutates(&ScreenAction::ListWindows));
    }

    #[test]
    fn validate_requires_element_id() {
        let args = ScreenArgs::default();
        assert!(validate(&ScreenAction::Invoke, &args).is_err());
        assert!(validate(&ScreenAction::Assert, &args).is_err());
        let mut ok = ScreenArgs::default();
        ok.element_id = Some("e1".to_string());
        ok.property = Some("name".to_string());
        ok.expected = Some("OK".to_string());
        assert!(validate(&ScreenAction::Assert, &ok).is_ok());
    }

    #[test]
    fn validate_click_needs_snapshot_and_coords() {
        let args = ScreenArgs::default();
        assert!(validate(&ScreenAction::Click, &args).is_err());
        let mut ok = ScreenArgs::default();
        ok.snapshot_id = Some("snap".to_string());
        ok.x = Some(10);
        ok.y = Some(20);
        assert!(validate(&ScreenAction::Click, &ok).is_ok());
        ok.x = Some(-1);
        assert!(validate(&ScreenAction::Click, &ok).is_err());
    }

    #[test]
    fn validate_type_limits() {
        let mut args = ScreenArgs::default();
        args.text = Some(String::new());
        assert!(validate(&ScreenAction::Type, &args).is_err()); // empty
        args.text = Some("x".repeat(4001));
        assert!(validate(&ScreenAction::Type, &args).is_err());
        args.text = Some("hello".to_string());
        assert!(validate(&ScreenAction::Type, &args).is_ok());
        // SetValue additionally requires an element_id (policy rule).
        args.element_id = Some("e1".to_string());
        args.text = Some("x".repeat(16001));
        assert!(validate(&ScreenAction::SetValue, &args).is_err());
        args.text = Some("x".repeat(16000));
        assert!(validate(&ScreenAction::SetValue, &args).is_ok());
    }

    #[test]
    fn validate_assert_options() {
        let mut args = ScreenArgs::default();
        args.element_id = Some("e1".to_string());
        args.property = Some("bogus".to_string());
        args.expected = Some("x".to_string());
        assert!(validate(&ScreenAction::Assert, &args).is_err());
        args.property = Some("enabled".to_string());
        args.match_ = Some("regex".to_string());
        assert!(validate(&ScreenAction::Assert, &args).is_err());
        args.match_ = Some("contains".to_string());
        args.wait_ms = Some(5001);
        assert!(validate(&ScreenAction::Assert, &args).is_err());
        args.wait_ms = Some(1000);
        assert!(validate(&ScreenAction::Assert, &args).is_ok());
    }

    #[test]
    fn validate_scroll_direction() {
        let mut args = ScreenArgs::default();
        args.element_id = Some("e1".to_string());
        args.direction = Some("diagonal".to_string());
        assert!(validate(&ScreenAction::Scroll, &args).is_err());
        args.direction = Some("down".to_string());
        assert!(validate(&ScreenAction::Scroll, &args).is_ok());
    }
}
