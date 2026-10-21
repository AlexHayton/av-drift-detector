//! Writing test files.

use ffmpeg::{ChannelLayout, Dictionary, Packet, Rational, codec, color, encoder, format, frame};
use ffmpeg_next as ffmpeg;

pub use super::{AudioCodec, GenerateOptions, Progress, VideoCodec};
use crate::audio::{ToneCode, ToneSynth};
use crate::pattern::PatternRenderer;
use crate::pattern::canvas::LumaCanvas;
use crate::{Error, Result};

/// Writes a test file: every video frame stamped with its number, every
/// frame's worth of audio carrying the same number as tones.
pub fn generate(opts: &GenerateOptions, mut on_progress: impl FnMut(Progress)) -> Result<()> {
    opts.validate()?;
    ffmpeg::init()?;
    let fps = opts.fps;
    let rate = opts.sample_rate;
    let tones = ToneCode::new(fps, rate)?;
    let renderer = PatternRenderer::new(opts.width as usize, opts.height as usize, tones.clone())?;
    let mut synth = ToneSynth::new(tones, opts.audio_delay);

    let mut octx = format::output(&opts.output)?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    // Video encoder and stream.
    let video_tb = Rational::new(fps.den() as i32, fps.num() as i32);
    let vcodec = match opts.video_codec {
        VideoCodec::H264 => encoder::find_by_name("libx264"),
        VideoCodec::Ffv1 => encoder::find(codec::Id::FFV1),
    }
    .ok_or_else(|| {
        Error::Unsupported(format!(
            "{:?} encoder missing from this FFmpeg build",
            opts.video_codec
        ))
    })?;
    let mut vctx = codec::context::Context::new_with_codec(vcodec)
        .encoder()
        .video()?;
    vctx.set_width(opts.width);
    vctx.set_height(opts.height);
    vctx.set_format(format::Pixel::YUV420P);
    vctx.set_color_range(color::Range::MPEG);
    vctx.set_colorspace(color::Space::BT709);
    vctx.set_time_base(video_tb);
    vctx.set_frame_rate(Some(fps));
    vctx.set_gop(fps.nominal());
    // Closed GOPs, so seeking to any keyframe (e.g. `analyze --offset`) decodes cleanly.
    let mut flags = codec::Flags::CLOSED_GOP;
    if global_header {
        flags |= codec::Flags::GLOBAL_HEADER;
    }
    vctx.set_flags(flags);
    let mut vopts = Dictionary::new();
    if opts.video_codec == VideoCodec::H264 {
        vopts.set("crf", "18");
        vopts.set("preset", "fast");
    }
    let mut venc = vctx.open_with(vopts)?;
    let vindex = {
        let mut st = octx.add_stream(vcodec)?;
        st.set_parameters(&venc);
        st.set_time_base(video_tb);
        st.set_avg_frame_rate(fps);
        st.index()
    };

    // Audio encoder and stream (mono).
    let (acodec_id, sample_format) = match opts.audio_codec {
        AudioCodec::Aac => (
            codec::Id::AAC,
            format::Sample::F32(format::sample::Type::Planar),
        ),
        AudioCodec::Pcm => (
            codec::Id::PCM_S16LE,
            format::Sample::I16(format::sample::Type::Packed),
        ),
    };
    let acodec = encoder::find(acodec_id).ok_or_else(|| {
        Error::Unsupported(format!(
            "{:?} encoder missing from this FFmpeg build",
            opts.audio_codec
        ))
    })?;
    let audio_tb = Rational::new(1, rate as i32);
    let mut actx = codec::context::Context::new_with_codec(acodec)
        .encoder()
        .audio()?;
    actx.set_rate(rate as i32);
    actx.set_channel_layout(ChannelLayout::MONO);
    actx.set_format(sample_format);
    actx.set_time_base(audio_tb);
    if opts.audio_codec == AudioCodec::Aac {
        actx.set_bit_rate(256_000);
    }
    if global_header {
        actx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut aenc = actx.open()?;
    let chunk = match aenc.frame_size() {
        0 => 1024,
        n => n as usize,
    };
    let aindex = {
        let mut st = octx.add_stream(acodec)?;
        st.set_parameters(&aenc);
        st.set_time_base(audio_tb);
        st.index()
    };

    octx.write_header()?;
    let video_stream_tb = octx.stream(vindex).expect("video stream").time_base();
    let audio_stream_tb = octx.stream(aindex).expect("audio stream").time_base();

    let total_frames = opts.total_frames();
    let total_samples = fps.frame_start_sample(total_frames as i64, rate) as u64;
    let mut pcm = vec![0.0f32; chunk];

    let mut write_audio = |synth: &mut ToneSynth,
                           aenc: &mut encoder::Audio,
                           octx: &mut format::context::Output,
                           upto: u64,
                           partial: bool|
     -> Result<()> {
        loop {
            let pos = synth.position();
            let remaining = upto.saturating_sub(pos) as usize;
            if remaining == 0 || (remaining < chunk && !partial) {
                return Ok(());
            }
            let count = remaining.min(chunk);
            synth.fill(&mut pcm[..count]);
            let mut af = frame::Audio::new(sample_format, count, ChannelLayout::MONO);
            af.set_rate(rate);
            match opts.audio_codec {
                AudioCodec::Aac => af.plane_mut::<f32>(0)[..count].copy_from_slice(&pcm[..count]),
                AudioCodec::Pcm => {
                    for (dst, &src) in af.plane_mut::<i16>(0)[..count]
                        .iter_mut()
                        .zip(&pcm[..count])
                    {
                        *dst = (src * 32767.0).round() as i16;
                    }
                }
            }
            af.set_pts(Some(pos as i64));
            aenc.send_frame(&af)?;
            drain(aenc, aindex, audio_tb, audio_stream_tb, octx)?;
        }
    };

    for n in 0..total_frames {
        let mut vf = frame::Video::new(format::Pixel::YUV420P, opts.width, opts.height);
        {
            let stride = vf.stride(0);
            let mut canvas = LumaCanvas::new(
                vf.data_mut(0),
                opts.width as usize,
                opts.height as usize,
                stride,
            );
            renderer.render(n as u32, &mut canvas)?;
        }
        vf.data_mut(1).fill(128);
        vf.data_mut(2).fill(128);
        vf.set_color_range(color::Range::MPEG);
        vf.set_pts(Some(n as i64));
        venc.send_frame(&vf)?;
        drain(&mut venc, vindex, video_tb, video_stream_tb, &mut octx)?;

        // Keep audio roughly level with video so the muxer can interleave.
        let upto = (fps.frame_start_sample(n as i64 + 1, rate) as u64).min(total_samples);
        write_audio(&mut synth, &mut aenc, &mut octx, upto, false)?;
        on_progress(Progress {
            frames_done: n + 1,
            frames_total: total_frames,
        });
    }
    write_audio(&mut synth, &mut aenc, &mut octx, total_samples, true)?;

    venc.send_eof()?;
    drain(&mut venc, vindex, video_tb, video_stream_tb, &mut octx)?;
    aenc.send_eof()?;
    drain(&mut aenc, aindex, audio_tb, audio_stream_tb, &mut octx)?;
    octx.write_trailer()?;
    Ok(())
}

fn drain(
    enc: &mut encoder::Encoder,
    stream: usize,
    encoder_tb: Rational,
    stream_tb: Rational,
    octx: &mut format::context::Output,
) -> Result<()> {
    let mut packet = Packet::empty();
    while enc.receive_packet(&mut packet).is_ok() {
        packet.set_stream(stream);
        packet.rescale_ts(encoder_tb, stream_tb);
        packet.write_interleaved(octx)?;
    }
    Ok(())
}
