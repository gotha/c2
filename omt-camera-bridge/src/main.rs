//! Reads raw YUV420 (I420) camera frames from c2/omtbridge.py over stdin -
//! c2 spawns this as a subprocess, the same way picamera2's FfmpegOutput
//! spawns ffmpeg - converts them to UYVY, and streams them out over Open
//! Media Transport (OMT) so they show up as a source in OBS/vMix/etc.
//!
//! Usage:
//!   omt-camera-bridge [--name NAME] [--width W] [--height H] [--fps N]

use std::io::{self, Read};
use std::time::{Duration, Instant};

use openmediatransport::{Codec, ColorSpace, Discovery, FrameType, MediaFrame, Sender};
use tracing_subscriber::layer::SubscriberExt;
use yuv::{BufferStoreMut, YuvPackedImageMut, YuvPlanarImage, yuv420_to_uyvy422};

mod metrics;
use metrics::Metrics;

struct Args {
    name: String,
    width: i32,
    height: i32,
    fps_n: i32,
    fps_d: i32,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            name: "C2 Camera".to_string(),
            width: 1920,
            height: 1080,
            fps_n: 30,
            fps_d: 1,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--name" => a.name = it.next().expect("--name needs a value"),
                "--width" => {
                    a.width = it
                        .next()
                        .expect("--width needs a value")
                        .parse()
                        .expect("bad --width")
                }
                "--height" => {
                    a.height = it
                        .next()
                        .expect("--height needs a value")
                        .parse()
                        .expect("bad --height")
                }
                "--fps" => {
                    a.fps_n = it
                        .next()
                        .expect("--fps needs a value")
                        .parse()
                        .expect("bad --fps")
                }
                other => eprintln!("omt-camera-bridge: ignoring unknown argument {other:?}"),
            }
        }
        a
    }
}

/// Assumes a tightly-packed I420 buffer (stride == width for Y, width/2 for
/// U/V) and an even width - what picamera2's "YUV420" format hands off.
fn i420_to_uyvy(i420: &[u8], width: usize, height: usize) -> Vec<u8> {
    let y_size = width * height;
    let c_w = width / 2;
    let c_h = height / 2;

    let planar = YuvPlanarImage {
        y_plane: &i420[..y_size],
        y_stride: width as u32,
        u_plane: &i420[y_size..y_size + c_w * c_h],
        u_stride: c_w as u32,
        v_plane: &i420[y_size + c_w * c_h..y_size + 2 * c_w * c_h],
        v_stride: c_w as u32,
        width: width as u32,
        height: height as u32,
    };

    let mut out = vec![0u8; width * height * 2];
    let mut packed = YuvPackedImageMut {
        yuy: BufferStoreMut::Borrowed(&mut out),
        yuy_stride: (width * 2) as u32,
        width: width as u32,
        height: height as u32,
    };

    yuv420_to_uyvy422(&mut packed, &planar).expect("yuv420_to_uyvy422");
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // No "F_" prefix on our own fields - we already namespace them with `omt_`.
    let journald_layer = tracing_journald::Layer::new()?.with_field_prefix(None);
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(journald_layer))?;

    let args = Args::parse();
    if args.width % 2 != 0 {
        return Err("--width must be even (UYVY is 4:2:2)".into());
    }

    let mut sender = Sender::create(&args.name, FrameType::VIDEO | FrameType::METADATA)?;
    let port = sender.port();
    let mut discovery = Discovery::new()?;
    discovery.register(&args.name, port)?;
    println!(
        "omt-camera-bridge: sending {:?} on port {port} ({}x{} @ {}/{} fps), reading I420 from stdin",
        args.name, args.width, args.height, args.fps_n, args.fps_d
    );

    let width = args.width as usize;
    let height = args.height as usize;
    let i420_size = width * height * 3 / 2;
    let mut i420_buf = vec![0u8; i420_size];

    let epoch = Instant::now();
    let mut last_sub = false;

    // Wall-clock: both stages parallelize internally, so low % != low CPU (check top/htop).
    let frame_budget = Duration::from_secs_f64(1.0 / args.fps_n.max(1) as f64);
    let mut metrics = Metrics::new(Duration::from_secs(5), frame_budget, args.fps_n);
    let mut stdin = io::stdin().lock();

    loop {
        if stdin.read_exact(&mut i420_buf).is_err() {
            println!("omt-camera-bridge: stdin closed, exiting");
            return Ok(());
        }

        sender.poll_accept()?;
        sender.poll_peer_metadata()?;

        let subscribed = sender.video_subscribed();
        if subscribed != last_sub {
            println!("omt-camera-bridge: video subscribed: {subscribed}");
            last_sub = subscribed;
        }
        if !subscribed {
            continue; // still drain stdin above so Python never blocks
        }

        let data = metrics.time_conversion(|| i420_to_uyvy(&i420_buf, width, height));
        metrics.record_frame();

        let frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            timestamp: (epoch.elapsed().as_micros() as i64) * 10, // 100ns units
            codec: Codec::Uyvy as i32,
            width: args.width,
            height: args.height,
            stride: args.width * 2,
            frame_rate_n: args.fps_n,
            frame_rate_d: args.fps_d,
            aspect_ratio: args.width as f32 / args.height.max(1) as f32,
            color_space: ColorSpace::Bt709,
            data,
            ..Default::default()
        };
        metrics.time_send(|| sender.send_video(frame))?;
        metrics.maybe_report();
    }
}
