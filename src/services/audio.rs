use futures_util::Stream;

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub volume: f32,
    pub muted: bool,
    pub icon_name: &'static str,
}

impl Default for AudioInfo {
    fn default() -> Self {
        Self {
            volume: 0.0,
            muted: false,
            icon_name: crate::style::ICON_VOLUME_OFF,
        }
    }
}

fn volume_icon(volume: f32, muted: bool) -> &'static str {
    if muted {
        return crate::style::ICON_VOLUME_OFF;
    }
    let pct = volume * 100.0;
    if pct >= 66.0 {
        crate::style::ICON_VOLUME_UP
    } else if pct >= 33.0 {
        crate::style::ICON_VOLUME_DOWN
    } else if pct >= 1.0 {
        crate::style::ICON_VOLUME_MUTE
    } else {
        crate::style::ICON_VOLUME_OFF
    }
}

async fn read_audio_wpctl() -> AudioInfo {
    let Ok(output) = tokio::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .await
    else {
        return AudioInfo::default();
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let muted = text.contains("[MUTED]");

    let volume = text
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.0);

    AudioInfo {
        volume,
        muted,
        icon_name: volume_icon(volume, muted),
    }
}

pub fn stream() -> impl Stream<Item = AudioInfo> {
    futures_util::stream::unfold((), |()| async {
        let info = read_audio_wpctl().await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Some((info, ()))
    })
}
