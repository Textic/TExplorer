//! Native Windows video playback engine built on Windows Media Foundation (`IMFMediaEngine`)
//! in frame-server mode with WIC BGRA frame transfer. Zero external DLLs or CLI dependencies.

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::com_shell::RawBgraImage;

/// A decoded BGRA video frame bundled with synchronized transport telemetry.
#[derive(Debug, Clone)]
pub struct VideoFrameSnapshot {
    pub image: RawBgraImage,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub playing: bool,
    pub muted: bool,
    pub volume: f64,
    pub native_width: u32,
    pub native_height: u32,
}

/// Lightweight playback status snapshot (emitted when state changes without a new video frame).
#[derive(Debug, Clone, Copy)]
pub struct VideoPlaybackStatus {
    pub position_secs: f64,
    pub duration_secs: f64,
    pub playing: bool,
    pub muted: bool,
    pub volume: f64,
    pub native_width: u32,
    pub native_height: u32,
}

/// Initial audio and looping configuration for a spawned video player session.
#[derive(Debug, Clone, Copy)]
pub struct VideoStartOptions {
    pub initial_muted: bool,
    pub initial_volume: f64,
    pub loop_playback: bool,
}

impl Default for VideoStartOptions {
    fn default() -> Self {
        Self {
            initial_muted: true,
            initial_volume: 0.7,
            loop_playback: true,
        }
    }
}

/// Notifications pushed from the background video engine to the UI thread.
#[derive(Debug, Clone)]
pub enum VideoNotify {
    Ready,
    FrameReady,
    StatusChanged(VideoPlaybackStatus),
    Error(String),
}

#[derive(Debug, Clone, Copy)]
enum VideoCommand {
    Play,
    Pause,
    TogglePlay,
    Seek(f64),
    SetMuted(bool),
    ToggleMute,
    SetVolume(f64),
    Stop,
}

/// Handle controlling a running background video playback session.
/// Dropping this handle immediately stops playback and releases the underlying file handle.
pub struct VideoPlayerHandle {
    cmd_tx: mpsc::Sender<VideoCommand>,
    latest_frame: Arc<Mutex<Option<VideoFrameSnapshot>>>,
    stopped: Arc<AtomicBool>,
}

impl VideoPlayerHandle {
    /// Takes the most recent decoded frame from the single-slot mailbox (preventing queue lag).
    pub fn take_latest_frame(&self) -> Option<VideoFrameSnapshot> {
        self.latest_frame.lock().ok().and_then(|mut slot| slot.take())
    }

    pub fn play(&self) {
        let _ = self.cmd_tx.send(VideoCommand::Play);
    }

    pub fn pause(&self) {
        let _ = self.cmd_tx.send(VideoCommand::Pause);
    }

    pub fn toggle_play(&self) {
        let _ = self.cmd_tx.send(VideoCommand::TogglePlay);
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.cmd_tx.send(VideoCommand::Seek(seconds));
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.cmd_tx.send(VideoCommand::SetMuted(muted));
    }

    pub fn toggle_mute(&self) {
        let _ = self.cmd_tx.send(VideoCommand::ToggleMute);
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.cmd_tx.send(VideoCommand::SetVolume(volume.clamp(0.0, 1.0)));
    }

    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            let _ = self.cmd_tx.send(VideoCommand::Stop);
        }
    }
}

impl Drop for VideoPlayerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Computes aspect-ratio-preserving target dimensions capped to `(max_w, max_h)`.
pub fn fit_video_dimensions(native_w: u32, native_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if native_w == 0 || native_h == 0 || max_w == 0 || max_h == 0 {
        return (2, 2);
    }
    if native_w <= max_w && native_h <= max_h {
        return (native_w.max(2), native_h.max(2));
    }
    let scale = (max_w as f64 / native_w as f64).min(max_h as f64 / native_h as f64);
    let w = ((native_w as f64 * scale).round() as u32).clamp(2, max_w);
    let h = ((native_h as f64 * scale).round() as u32).clamp(2, max_h);
    (w, h)
}

/// Spawns a dedicated background video playback worker for `path`.
pub fn spawn_video_player(
    path: PathBuf,
    options: VideoStartOptions,
    on_notify: impl Fn(VideoNotify) + Send + 'static,
) -> Result<VideoPlayerHandle, String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::spawn_mf_video_player(path, options, on_notify)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, options, on_notify);
        Err("Inline video playback is currently supported on Windows only.".to_string())
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::path::PathBuf;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{RECT, S_OK};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmap, IWICImagingFactory,
        WICBitmapCacheOnLoad, WICBitmapLockRead, WICRect,
    };
    use windows::Win32::Media::MediaFoundation::{
        CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine,
        IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl,
        MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_EVENT, MF_MEDIA_ENGINE_EVENT_CANPLAY,
        MF_MEDIA_ENGINE_EVENT_CANPLAYTHROUGH, MF_MEDIA_ENGINE_EVENT_ENDED,
        MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
        MF_MEDIA_ENGINE_EVENT_SEEKED, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
        MFCreateAttributes, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::core::{BSTR, Interface, implement};

    use super::{
        VideoCommand, VideoFrameSnapshot, VideoNotify, VideoPlaybackStatus, VideoPlayerHandle,
        VideoStartOptions, fit_video_dimensions,
    };
    use crate::com_shell::RawBgraImage;

    const MAX_VIDEO_RENDER_W: u32 = 960;
    const MAX_VIDEO_RENDER_H: u32 = 560;
    const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(32);

    struct CallbackFlags {
        loaded_metadata: AtomicBool,
        can_play: AtomicBool,
        seeked: AtomicBool,
        ended: AtomicBool,
        error: AtomicBool,
    }

    impl CallbackFlags {
        fn new() -> Self {
            Self {
                loaded_metadata: AtomicBool::new(false),
                can_play: AtomicBool::new(false),
                seeked: AtomicBool::new(false),
                ended: AtomicBool::new(false),
                error: AtomicBool::new(false),
            }
        }
    }

    #[implement(IMFMediaEngineNotify)]
    struct MediaEngineCallback {
        flags: Arc<CallbackFlags>,
    }

    impl IMFMediaEngineNotify_Impl for MediaEngineCallback_Impl {
        fn EventNotify(
            &self,
            event: u32,
            _param1: usize,
            _param2: u32,
        ) -> windows::core::Result<()> {
            match MF_MEDIA_ENGINE_EVENT(event as i32) {
                MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA => {
                    self.flags.loaded_metadata.store(true, Ordering::Release);
                }
                MF_MEDIA_ENGINE_EVENT_CANPLAY | MF_MEDIA_ENGINE_EVENT_CANPLAYTHROUGH => {
                    self.flags.can_play.store(true, Ordering::Release);
                }
                MF_MEDIA_ENGINE_EVENT_SEEKED => {
                    self.flags.seeked.store(true, Ordering::Release);
                }
                MF_MEDIA_ENGINE_EVENT_ENDED => {
                    self.flags.ended.store(true, Ordering::Release);
                }
                MF_MEDIA_ENGINE_EVENT_ERROR => {
                    self.flags.error.store(true, Ordering::Release);
                }
                _ => {}
            }
            Ok(())
        }
    }

    pub fn spawn_mf_video_player(
        path: PathBuf,
        options: VideoStartOptions,
        on_notify: impl Fn(VideoNotify) + Send + 'static,
    ) -> Result<VideoPlayerHandle, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<VideoCommand>();
        let latest_frame: Arc<Mutex<Option<VideoFrameSnapshot>>> = Arc::new(Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));

        let worker_frame = Arc::clone(&latest_frame);
        let worker_stopped = Arc::clone(&stopped);

        std::thread::Builder::new()
            .name("texplorer-video".to_string())
            .spawn(move || {
                if let Err(err) =
                    run_mf_worker(path, options, cmd_rx, worker_frame, worker_stopped, &on_notify)
                {
                    on_notify(VideoNotify::Error(err));
                }
            })
            .map_err(|e| format!("Failed to spawn video playback thread: {e}"))?;

        Ok(VideoPlayerHandle {
            cmd_tx,
            latest_frame,
            stopped,
        })
    }

    /// Calls `IMFMediaEngine::OnVideoStreamTick` and returns `true` ONLY when Media Foundation
    /// explicitly returns `S_OK` (0) indicating a fresh video frame is ready for transfer.
    unsafe fn media_engine_has_new_frame(engine: &IMFMediaEngine) -> bool {
        let mut pts: i64 = 0;
        let hr = unsafe {
            (Interface::vtable(engine).OnVideoStreamTick)(Interface::as_raw(engine), &mut pts)
        };
        hr == S_OK && pts >= 0
    }

    fn safe_duration(engine: &IMFMediaEngine) -> f64 {
        let raw = unsafe { engine.GetDuration() };
        if raw.is_finite() && raw > 0.0 {
            raw
        } else {
            0.0
        }
    }

    fn safe_position(engine: &IMFMediaEngine, duration: f64) -> f64 {
        let raw = unsafe { engine.GetCurrentTime() };
        if raw.is_finite() && raw >= 0.0 {
            if duration > 0.0 {
                raw.min(duration)
            } else {
                raw
            }
        } else {
            0.0
        }
    }

    /// Transfers the current frame from `engine` into `bitmap` and extracts tightly packed opaque BGRA pixels.
    unsafe fn capture_video_frame(
        engine: &IMFMediaEngine,
        bitmap: &IWICBitmap,
        width: u32,
        height: u32,
    ) -> Option<RawBgraImage> {
        let dst_rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        unsafe {
            engine
                .TransferVideoFrame(bitmap, None, &dst_rect, None)
                .ok()?;
        }

        let lock_rect = WICRect {
            X: 0,
            Y: 0,
            Width: width as i32,
            Height: height as i32,
        };
        let lock = unsafe { bitmap.Lock(&lock_rect, WICBitmapLockRead.0 as u32).ok()? };
        let stride = unsafe { lock.GetStride().ok()? } as usize;
        let mut buf_size: u32 = 0;
        let mut data_ptr: *mut u8 = std::ptr::null_mut();
        unsafe {
            lock.GetDataPointer(&mut buf_size, &mut data_ptr).ok()?;
        }
        if data_ptr.is_null() {
            return None;
        }

        let row_bytes = (width as usize).checked_mul(4)?;
        let total_needed = stride.checked_mul(height as usize)?;
        if (buf_size as usize) < total_needed || stride < row_bytes {
            return None;
        }

        let src = unsafe { std::slice::from_raw_parts(data_ptr, buf_size as usize) };
        let mut bgra = vec![0u8; row_bytes * (height as usize)];
        for y in 0..(height as usize) {
            let src_row = &src[y * stride..y * stride + row_bytes];
            let dst_row = &mut bgra[y * row_bytes..(y + 1) * row_bytes];
            dst_row.copy_from_slice(src_row);
            // Hardware video decoders frequently emit alpha = 0 for opaque video frames;
            // force full opacity so GPUI always composites the frame visibly.
            for px in dst_row.as_chunks_mut::<4>().0 {
                px[3] = 255;
            }
        }

        Some(RawBgraImage {
            width,
            height,
            bgra,
        })
    }

    fn run_mf_worker(
        path: PathBuf,
        options: VideoStartOptions,
        cmd_rx: mpsc::Receiver<VideoCommand>,
        latest_frame: Arc<Mutex<Option<VideoFrameSnapshot>>>,
        stopped: Arc<AtomicBool>,
        on_notify: &(impl Fn(VideoNotify) + Send),
    ) -> Result<(), String> {
        let co_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let com_inited = co_hr.is_ok();
        if !com_inited {
            return Err("Failed to initialize COM for video playback.".to_string());
        }

        let mf_res = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) };
        if let Err(err) = mf_res {
            unsafe { CoUninitialize() };
            return Err(format!("Failed to initialize Windows Media Foundation: {err}"));
        }

        let run_result = run_mf_loop(path, options, cmd_rx, latest_frame, stopped, on_notify);

        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
        run_result
    }

    fn run_mf_loop(
        path: PathBuf,
        options: VideoStartOptions,
        cmd_rx: mpsc::Receiver<VideoCommand>,
        latest_frame: Arc<Mutex<Option<VideoFrameSnapshot>>>,
        stopped: Arc<AtomicBool>,
        on_notify: &(impl Fn(VideoNotify) + Send),
    ) -> Result<(), String> {
        let flags = Arc::new(CallbackFlags::new());
        let notify: IMFMediaEngineNotify = MediaEngineCallback {
            flags: Arc::clone(&flags),
        }
        .into();

        let mut attrs_opt: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attrs_opt, 2) }
            .map_err(|e| format!("Failed to create MF attributes: {e}"))?;
        let attrs = attrs_opt.ok_or_else(|| "MFCreateAttributes returned null.".to_string())?;

        unsafe {
            attrs
                .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
                .map_err(|e| format!("Failed to set MF callback: {e}"))?;
            attrs
                .SetUINT32(
                    &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                    DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
                )
                .map_err(|e| format!("Failed to set BGRA output format: {e}"))?;
        }

        let factory: IMFMediaEngineClassFactory = unsafe {
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(|e| format!("Failed to create MFMediaEngineClassFactory: {e}"))?;

        let engine: IMFMediaEngine = unsafe { factory.CreateInstance(0, &attrs) }
            .map_err(|e| format!("Failed to create IMFMediaEngine: {e}"))?;

        let wic_factory: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| format!("Failed to create WIC imaging factory: {e}"))?;

        let initial_vol = options.initial_volume.clamp(0.0, 1.0);
        let initial_muted = options.initial_muted;
        let loop_playback = options.loop_playback;

        let raw_path = path.to_string_lossy();
        let clean_path = raw_path.strip_prefix(r"\\?\").unwrap_or(&raw_path);
        let url_bstr = BSTR::from(clean_path);
        unsafe {
            let _ = engine.SetLoop(loop_playback);
            let _ = engine.SetVolume(initial_vol);
            let _ = engine.SetMuted(initial_muted);
            engine
                .SetSource(&url_bstr)
                .map_err(|e| format!("Media Foundation could not open video source: {e}"))?;
            let _ = engine.Load();
        }

        let mut wic_bitmap: Option<(IWICBitmap, u32, u32)> = None;
        let mut native_w: u32 = 0;
        let mut native_h: u32 = 0;
        let mut duration_secs: f64 = 0.0;
        let mut playing = false;
        let mut muted = initial_muted;
        let mut volume = initial_vol;
        let mut ended = false;
        let mut started_autoplay = false;
        let mut seek_pending_ticks: u32 = 0;
        let mut force_transfer = false;
        let mut last_frame_at = Instant::now() - Duration::from_secs(1);
        let mut last_status_at = Instant::now();
        let mut err_out: Option<String> = None;

        while !stopped.load(Ordering::Acquire) {
            let wait_ms = if playing || seek_pending_ticks > 0 || !started_autoplay {
                16
            } else {
                45
            };

            // Wait briefly for commands and coalesce bursty seeks (e.g. while dragging scrubber).
            let mut pending_cmds = Vec::new();
            match cmd_rx.recv_timeout(Duration::from_millis(wait_ms)) {
                Ok(cmd) => {
                    pending_cmds.push(cmd);
                    while let Ok(next) = cmd_rx.try_recv() {
                        pending_cmds.push(next);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let mut should_stop = false;
            let mut coalesced_seek: Option<f64> = None;
            let mut status_dirty = false;

            for cmd in pending_cmds {
                match cmd {
                    VideoCommand::Stop => {
                        should_stop = true;
                        break;
                    }
                    VideoCommand::Play => {
                        let cur_pos = safe_position(&engine, duration_secs);
                        if ended || (duration_secs > 0.0 && cur_pos >= duration_secs - 0.15) {
                            let _ = unsafe { engine.SetCurrentTime(0.0) };
                        }
                        if unsafe { engine.Play() }.is_ok() {
                            playing = true;
                            ended = false;
                            status_dirty = true;
                        }
                    }
                    VideoCommand::Pause => {
                        let _ = unsafe { engine.Pause() };
                        playing = false;
                        status_dirty = true;
                    }
                    VideoCommand::TogglePlay => {
                        if playing {
                            let _ = unsafe { engine.Pause() };
                            playing = false;
                        } else {
                            let cur_pos = safe_position(&engine, duration_secs);
                            if ended || (duration_secs > 0.0 && cur_pos >= duration_secs - 0.15) {
                                let _ = unsafe { engine.SetCurrentTime(0.0) };
                            }
                            if unsafe { engine.Play() }.is_ok() {
                                playing = true;
                                ended = false;
                            }
                        }
                        status_dirty = true;
                    }
                    VideoCommand::Seek(secs) => {
                        coalesced_seek = Some(secs);
                    }
                    VideoCommand::SetMuted(m) => {
                        let _ = unsafe { engine.SetMuted(m) };
                        muted = m;
                        status_dirty = true;
                    }
                    VideoCommand::ToggleMute => {
                        muted = !muted;
                        let _ = unsafe { engine.SetMuted(muted) };
                        status_dirty = true;
                    }
                    VideoCommand::SetVolume(v) => {
                        volume = v.clamp(0.0, 1.0);
                        let _ = unsafe { engine.SetVolume(volume) };
                        status_dirty = true;
                    }
                }
            }

            if should_stop || stopped.load(Ordering::Acquire) {
                break;
            }

            if let Some(target_secs) = coalesced_seek {
                let max_t = if duration_secs > 0.0 {
                    duration_secs
                } else {
                    safe_duration(&engine)
                };
                let clamped = if max_t > 0.0 {
                    target_secs.clamp(0.0, max_t)
                } else {
                    target_secs.max(0.0)
                };
                let _ = unsafe { engine.SetCurrentTime(clamped) };
                ended = false;
                seek_pending_ticks = 24;
                force_transfer = true;
                status_dirty = true;
            }

            if flags.error.swap(false, Ordering::AcqRel) {
                err_out = Some(
                    "Codec not supported for inline playback — click Open to launch default player."
                        .to_string(),
                );
                break;
            }

            if flags.loaded_metadata.swap(false, Ordering::AcqRel)
                || (wic_bitmap.is_none() && flags.can_play.load(Ordering::Acquire))
            {
                duration_secs = safe_duration(&engine);
                let mut nw = 0u32;
                let mut nh = 0u32;
                if unsafe { engine.GetNativeVideoSize(Some(&mut nw), Some(&mut nh)) }.is_ok()
                    && nw > 0
                    && nh > 0
                {
                    native_w = nw;
                    native_h = nh;
                    let (dst_w, dst_h) =
                        fit_video_dimensions(nw, nh, MAX_VIDEO_RENDER_W, MAX_VIDEO_RENDER_H);
                    if let Ok(bmp) = unsafe {
                        wic_factory.CreateBitmap(
                            dst_w,
                            dst_h,
                            &GUID_WICPixelFormat32bppBGRA,
                            WICBitmapCacheOnLoad,
                        )
                    } {
                        wic_bitmap = Some((bmp, dst_w, dst_h));
                    }
                }
                status_dirty = true;
            }

            if !started_autoplay && flags.can_play.load(Ordering::Acquire) {
                started_autoplay = true;
                duration_secs = safe_duration(&engine);
                unsafe {
                    let _ = engine.SetLoop(loop_playback);
                    let _ = engine.SetVolume(volume);
                    let _ = engine.SetMuted(muted);
                }
                if unsafe { engine.Play() }.is_ok() {
                    playing = true;
                }
                seek_pending_ticks = 15;
                force_transfer = true;
                on_notify(VideoNotify::Ready);
                status_dirty = true;
            }

            if flags.seeked.swap(false, Ordering::AcqRel) {
                force_transfer = true;
                seek_pending_ticks = seek_pending_ticks.max(6);
            }

            if flags.ended.swap(false, Ordering::AcqRel) {
                if loop_playback {
                    let _ = unsafe { engine.SetCurrentTime(0.0) };
                    let _ = unsafe { engine.Play() };
                    playing = true;
                    ended = false;
                    force_transfer = true;
                } else {
                    playing = false;
                    ended = true;
                }
                status_dirty = true;
            }

            if duration_secs <= 0.0 {
                let d = safe_duration(&engine);
                if d > 0.0 {
                    duration_secs = d;
                }
            }

            let tick_ready = unsafe { media_engine_has_new_frame(&engine) };
            let should_capture = if let Some((_, _, _)) = wic_bitmap.as_ref() {
                (tick_ready && last_frame_at.elapsed() >= MIN_FRAME_INTERVAL)
                    || (force_transfer && tick_ready)
                    || (force_transfer && !playing)
            } else {
                false
            };

            if should_capture
                && let Some((ref bmp, dst_w, dst_h)) = wic_bitmap
                && let Some(raw_img) = unsafe { capture_video_frame(&engine, bmp, dst_w, dst_h) }
            {
                let pos = safe_position(&engine, duration_secs);
                let snapshot = VideoFrameSnapshot {
                    image: raw_img,
                    position_secs: pos,
                    duration_secs,
                    playing,
                    muted,
                    volume,
                    native_width: native_w,
                    native_height: native_h,
                };
                let should_signal = if let Ok(mut slot) = latest_frame.lock() {
                    let was_empty = slot.is_none();
                    *slot = Some(snapshot);
                    was_empty
                } else {
                    false
                };
                if should_signal {
                    on_notify(VideoNotify::FrameReady);
                }
                last_frame_at = Instant::now();
                last_status_at = Instant::now();
                force_transfer = false;
                seek_pending_ticks = seek_pending_ticks.saturating_sub(1);
                status_dirty = false;
            } else if seek_pending_ticks > 0 {
                seek_pending_ticks = seek_pending_ticks.saturating_sub(1);
            }

            if status_dirty || (playing && last_status_at.elapsed() >= Duration::from_millis(120)) {
                let pos = safe_position(&engine, duration_secs);
                on_notify(VideoNotify::StatusChanged(VideoPlaybackStatus {
                    position_secs: pos,
                    duration_secs,
                    playing,
                    muted,
                    volume,
                    native_width: native_w,
                    native_height: native_h,
                }));
                last_status_at = Instant::now();
            }
        }

        // Explicitly shut down the Media Foundation engine so the video file handle is released immediately.
        unsafe {
            let _ = engine.Pause();
            let _ = engine.Shutdown();
        }
        drop(wic_bitmap);
        drop(engine);
        drop(factory);
        drop(attrs);
        drop(notify);
        drop(wic_factory);

        if let Some(err) = err_out {
            Err(err)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_video_dimensions_preserving_aspect_ratio() {
        assert_eq!(fit_video_dimensions(1920, 1080, 960, 560), (960, 540));
        assert_eq!(fit_video_dimensions(1080, 1920, 960, 560), (315, 560));
        assert_eq!(fit_video_dimensions(640, 360, 960, 560), (640, 360));
        assert_eq!(fit_video_dimensions(0, 0, 960, 560), (2, 2));
    }
}
