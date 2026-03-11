use std::io::Cursor;

use png::{BlendOp, ColorType, DisposeOp, Transformations};

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApngDefaultImage {
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApngFrame {
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub delay_num: u16,
    pub delay_den: u16,
    pub dispose_op: DisposeOp,
    pub blend_op: BlendOp,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApngImage {
    pub width: u32,
    pub height: u32,
    pub num_plays: u32,
    pub default_image: Option<ApngDefaultImage>,
    pub frames: Vec<ApngFrame>,
}

pub fn decode_apng(input: &[u8]) -> Result<Option<ApngImage>, AppError> {
    let mut decoder = png::Decoder::new(Cursor::new(input));
    decoder.set_transformations(Transformations::normalize_to_color8() | Transformations::ALPHA);
    let mut reader = decoder
        .read_info()
        .map_err(|e| AppError::Decode(format!("failed to read PNG info: {e}")))?;

    let info = reader.info();
    let animation = info.animation_control().copied();
    let first_frame_control = info.frame_control().copied();
    let width = info.width;
    let height = info.height;
    let Some(animation) = animation else {
        return Ok(None);
    };
    let num_plays = animation.num_plays;
    let has_separate_default_image = first_frame_control.is_none();
    let buffer_len = reader
        .output_buffer_size()
        .ok_or_else(|| AppError::Decode("failed to compute APNG output buffer size".to_string()))?;
    let mut buffer = vec![0_u8; buffer_len];

    let default_image = if has_separate_default_image {
        let output = reader
            .next_frame(&mut buffer)
            .map_err(|e| AppError::Decode(format!("failed to decode APNG default image: {e}")))?;
        validate_rgba8_output(&output)?;
        if output.width != width || output.height != height {
            return Err(AppError::Decode(format!(
                "invalid APNG default image bounds: expected={}x{}, actual={}x{}",
                width, height, output.width, output.height
            )));
        }
        Some(ApngDefaultImage {
            rgba: buffer[..output.buffer_size()].to_vec(),
        })
    } else {
        None
    };

    let mut frames = Vec::with_capacity(animation.num_frames as usize);
    for index in 0..animation.num_frames {
        let output = reader.next_frame(&mut buffer).map_err(|e| {
            AppError::Decode(format!("failed to decode APNG frame {}: {e}", index + 1))
        })?;
        validate_rgba8_output(&output)?;
        let frame_control = *reader.info().frame_control().ok_or_else(|| {
            AppError::Decode(format!(
                "APNG frame {} missing frame control metadata",
                index + 1
            ))
        })?;
        let rgba = buffer[..output.buffer_size()].to_vec();
        validate_frame_bounds(
            width,
            height,
            output.width,
            output.height,
            frame_control.x_offset,
            frame_control.y_offset,
        )?;
        frames.push(ApngFrame {
            width: output.width,
            height: output.height,
            x_offset: frame_control.x_offset,
            y_offset: frame_control.y_offset,
            delay_num: frame_control.delay_num,
            delay_den: frame_control.delay_den,
            dispose_op: frame_control.dispose_op,
            blend_op: frame_control.blend_op,
            rgba,
        });
    }

    Ok(Some(ApngImage {
        width,
        height,
        num_plays,
        default_image,
        frames,
    }))
}

pub fn compose_frames(apng: &ApngImage) -> Result<Vec<Vec<u8>>, AppError> {
    let canvas_len = rgba_len(apng.width, apng.height)?;
    let mut canvas = vec![0_u8; canvas_len];
    let mut saved_before_previous: Option<Vec<u8>> = None;
    let mut previous_frame: Option<&ApngFrame> = None;
    let mut outputs = Vec::with_capacity(apng.frames.len());

    for (index, frame) in apng.frames.iter().enumerate() {
        validate_frame(apng.width, apng.height, frame)?;

        if let Some(prev) = previous_frame {
            match effective_dispose(prev, index - 1) {
                DisposeOp::None => {}
                DisposeOp::Background => clear_region(
                    &mut canvas,
                    apng.width,
                    prev.x_offset,
                    prev.y_offset,
                    prev.width,
                    prev.height,
                )?,
                DisposeOp::Previous => {
                    if let Some(saved) = saved_before_previous.take() {
                        canvas = saved;
                    } else {
                        clear_region(
                            &mut canvas,
                            apng.width,
                            prev.x_offset,
                            prev.y_offset,
                            prev.width,
                            prev.height,
                        )?;
                    }
                }
            }
        }

        if effective_dispose(frame, index) == DisposeOp::Previous {
            saved_before_previous = Some(canvas.clone());
        } else {
            saved_before_previous = None;
        }

        blend_frame(&mut canvas, apng.width, frame)?;
        outputs.push(canvas.clone());
        previous_frame = Some(frame);
    }

    Ok(outputs)
}

pub fn encode_apng(apng: &ApngImage) -> Result<Vec<u8>, AppError> {
    if apng.frames.is_empty() {
        return Err(AppError::Encode(
            "APNG encoding requires at least one animation frame".to_string(),
        ));
    }

    if apng.default_image.is_none() {
        let first = &apng.frames[0];
        if first.x_offset != 0
            || first.y_offset != 0
            || first.width != apng.width
            || first.height != apng.height
        {
            return Err(AppError::Encode(
                "first APNG frame must cover the full canvas when no separate default image is present"
                    .to_string(),
            ));
        }
    }

    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, apng.width, apng.height);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .set_animated(apng.frames.len() as u32, apng.num_plays)
        .map_err(|e| AppError::Encode(format!("failed to configure APNG animation: {e}")))?;

    if apng.default_image.is_some() {
        encoder.set_sep_def_img(true).map_err(|e| {
            AppError::Encode(format!("failed to configure APNG default image: {e}"))
        })?;
    }

    let mut writer = encoder
        .write_header()
        .map_err(|e| AppError::Encode(format!("failed to write APNG header: {e}")))?;

    if let Some(default_image) = &apng.default_image {
        if default_image.rgba.len() != rgba_len(apng.width, apng.height)? {
            return Err(AppError::Encode(format!(
                "APNG default image length mismatch: expected={}, actual={}",
                rgba_len(apng.width, apng.height)?,
                default_image.rgba.len()
            )));
        }
        writer
            .write_image_data(&default_image.rgba)
            .map_err(|e| AppError::Encode(format!("failed to write APNG default image: {e}")))?;
    }

    for frame in &apng.frames {
        validate_frame(apng.width, apng.height, frame)?;
        writer
            .reset_frame_position()
            .map_err(|e| AppError::Encode(format!("failed to reset APNG frame position: {e}")))?;
        writer
            .reset_frame_dimension()
            .map_err(|e| AppError::Encode(format!("failed to reset APNG frame dimension: {e}")))?;
        writer
            .set_frame_dimension(frame.width, frame.height)
            .map_err(|e| AppError::Encode(format!("failed to set APNG frame dimension: {e}")))?;
        writer
            .set_frame_position(frame.x_offset, frame.y_offset)
            .map_err(|e| AppError::Encode(format!("failed to set APNG frame position: {e}")))?;
        writer
            .set_frame_delay(frame.delay_num, frame.delay_den)
            .map_err(|e| AppError::Encode(format!("failed to set APNG frame delay: {e}")))?;
        writer
            .set_blend_op(frame.blend_op)
            .map_err(|e| AppError::Encode(format!("failed to set APNG blend op: {e}")))?;
        writer
            .set_dispose_op(frame.dispose_op)
            .map_err(|e| AppError::Encode(format!("failed to set APNG dispose op: {e}")))?;
        writer
            .write_image_data(&frame.rgba)
            .map_err(|e| AppError::Encode(format!("failed to write APNG frame data: {e}")))?;
    }

    writer
        .finish()
        .map_err(|e| AppError::Encode(format!("failed to finish APNG encoding: {e}")))?;
    Ok(output)
}

#[derive(Debug, Clone)]
pub struct IndexedApngFrame {
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub delay_num: u16,
    pub delay_den: u16,
    pub dispose_op: DisposeOp,
    pub blend_op: BlendOp,
    pub indices: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct IndexedApngImage {
    pub width: u32,
    pub height: u32,
    pub num_plays: u32,
    pub palette: Vec<[u8; 4]>,
    pub default_image_indices: Option<Vec<u8>>,
    pub frames: Vec<IndexedApngFrame>,
}

pub fn encode_indexed_apng(image: &IndexedApngImage) -> Result<Vec<u8>, AppError> {
    if image.frames.is_empty() {
        return Err(AppError::Encode(
            "indexed APNG encoding requires at least one animation frame".to_string(),
        ));
    }

    if image.default_image_indices.is_none() {
        let first = &image.frames[0];
        if first.x_offset != 0
            || first.y_offset != 0
            || first.width != image.width
            || first.height != image.height
        {
            return Err(AppError::Encode(
                "first indexed APNG frame must cover the full canvas when no separate default image is present"
                    .to_string(),
            ));
        }
    }

    let palette_len = image.palette.len();
    let plte: Vec<u8> = image.palette.iter().flat_map(|c| [c[0], c[1], c[2]]).collect();
    let trns: Vec<u8> = if let Some(last_non_opaque) = image.palette.iter().rposition(|c| c[3] < 255) {
        image.palette.iter().take(last_non_opaque + 1).map(|c| c[3]).collect()
    } else {
        Vec::new()
    };

    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, image.width, image.height);
    encoder.set_color(ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(plte);
    if !trns.is_empty() {
        encoder.set_trns(trns);
    }
    encoder
        .set_animated(image.frames.len() as u32, image.num_plays)
        .map_err(|e| AppError::Encode(format!("failed to configure indexed APNG animation: {e}")))?;

    if image.default_image_indices.is_some() {
        encoder.set_sep_def_img(true).map_err(|e| {
            AppError::Encode(format!("failed to configure indexed APNG default image: {e}"))
        })?;
    }

    let mut writer = encoder
        .write_header()
        .map_err(|e| AppError::Encode(format!("failed to write indexed APNG header: {e}")))?;

    if let Some(default_indices) = &image.default_image_indices {
        let expected = (image.width as usize) * (image.height as usize);
        if default_indices.len() != expected {
            return Err(AppError::Encode(format!(
                "indexed APNG default image length mismatch: expected={expected}, actual={}",
                default_indices.len()
            )));
        }
        for idx in default_indices {
            if (*idx as usize) >= palette_len {
                return Err(AppError::Encode(format!(
                    "indexed APNG default image palette index out of range: {idx}"
                )));
            }
        }
        writer
            .write_image_data(default_indices)
            .map_err(|e| AppError::Encode(format!("failed to write indexed APNG default image: {e}")))?;
    }

    for frame in &image.frames {
        let expected = (frame.width as usize) * (frame.height as usize);
        if frame.indices.len() != expected {
            return Err(AppError::Encode(format!(
                "indexed APNG frame length mismatch: expected={expected}, actual={}",
                frame.indices.len()
            )));
        }
        writer
            .reset_frame_position()
            .map_err(|e| AppError::Encode(format!("failed to reset indexed APNG frame position: {e}")))?;
        writer
            .reset_frame_dimension()
            .map_err(|e| AppError::Encode(format!("failed to reset indexed APNG frame dimension: {e}")))?;
        writer
            .set_frame_dimension(frame.width, frame.height)
            .map_err(|e| AppError::Encode(format!("failed to set indexed APNG frame dimension: {e}")))?;
        writer
            .set_frame_position(frame.x_offset, frame.y_offset)
            .map_err(|e| AppError::Encode(format!("failed to set indexed APNG frame position: {e}")))?;
        writer
            .set_frame_delay(frame.delay_num, frame.delay_den)
            .map_err(|e| AppError::Encode(format!("failed to set indexed APNG frame delay: {e}")))?;
        writer
            .set_blend_op(frame.blend_op)
            .map_err(|e| AppError::Encode(format!("failed to set indexed APNG blend op: {e}")))?;
        writer
            .set_dispose_op(frame.dispose_op)
            .map_err(|e| AppError::Encode(format!("failed to set indexed APNG dispose op: {e}")))?;
        writer
            .write_image_data(&frame.indices)
            .map_err(|e| AppError::Encode(format!("failed to write indexed APNG frame data: {e}")))?;
    }

    writer
        .finish()
        .map_err(|e| AppError::Encode(format!("failed to finish indexed APNG encoding: {e}")))?;
    Ok(output)
}

fn validate_rgba8_output(output: &png::OutputInfo) -> Result<(), AppError> {
    if output.color_type != ColorType::Rgba || output.bit_depth != png::BitDepth::Eight {
        return Err(AppError::Decode(format!(
            "unsupported APNG output format: {:?} {:?}",
            output.color_type, output.bit_depth
        )));
    }
    Ok(())
}

fn validate_frame(
    canvas_width: u32,
    canvas_height: u32,
    frame: &ApngFrame,
) -> Result<(), AppError> {
    validate_frame_bounds(
        canvas_width,
        canvas_height,
        frame.width,
        frame.height,
        frame.x_offset,
        frame.y_offset,
    )?;
    let expected_len = rgba_len(frame.width, frame.height)?;
    if frame.rgba.len() != expected_len {
        return Err(AppError::Encode(format!(
            "APNG frame length mismatch: expected={}, actual={}",
            expected_len,
            frame.rgba.len()
        )));
    }
    Ok(())
}

fn validate_frame_bounds(
    canvas_width: u32,
    canvas_height: u32,
    frame_width: u32,
    frame_height: u32,
    x_offset: u32,
    y_offset: u32,
) -> Result<(), AppError> {
    if frame_width == 0 || frame_height == 0 {
        return Err(AppError::Decode(
            "APNG frame dimensions must be non-zero".to_string(),
        ));
    }
    let in_x_bounds = Some(frame_width) <= canvas_width.checked_sub(x_offset);
    let in_y_bounds = Some(frame_height) <= canvas_height.checked_sub(y_offset);
    if !in_x_bounds || !in_y_bounds {
        return Err(AppError::Decode(format!(
            "APNG frame out of bounds: frame={}x{} at {},{} on canvas {}x{}",
            frame_width, frame_height, x_offset, y_offset, canvas_width, canvas_height
        )));
    }
    Ok(())
}

fn rgba_len(width: u32, height: u32) -> Result<usize, AppError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| AppError::Decode("APNG dimensions overflow RGBA buffer size".to_string()))
}

fn effective_dispose(frame: &ApngFrame, frame_index: usize) -> DisposeOp {
    if frame_index == 0 && frame.dispose_op == DisposeOp::Previous {
        DisposeOp::Background
    } else {
        frame.dispose_op
    }
}

fn clear_region(
    canvas: &mut [u8],
    canvas_width: u32,
    x_offset: u32,
    y_offset: u32,
    width: u32,
    height: u32,
) -> Result<(), AppError> {
    for y in y_offset..(y_offset + height) {
        for x in x_offset..(x_offset + width) {
            let idx = rgba_index(canvas_width, x, y)?;
            canvas[idx..idx + 4].fill(0);
        }
    }
    Ok(())
}

fn blend_frame(canvas: &mut [u8], canvas_width: u32, frame: &ApngFrame) -> Result<(), AppError> {
    for local_y in 0..frame.height {
        for local_x in 0..frame.width {
            let src_idx = rgba_index(frame.width, local_x, local_y)?;
            let dst_idx = rgba_index(
                canvas_width,
                frame.x_offset + local_x,
                frame.y_offset + local_y,
            )?;
            let src = &frame.rgba[src_idx..src_idx + 4];
            let dst = &mut canvas[dst_idx..dst_idx + 4];
            match frame.blend_op {
                BlendOp::Source => dst.copy_from_slice(src),
                BlendOp::Over => blend_over(dst, src),
            }
        }
    }
    Ok(())
}

fn rgba_index(width: u32, x: u32, y: u32) -> Result<usize, AppError> {
    let pixel_index = (y as usize)
        .checked_mul(width as usize)
        .and_then(|row| row.checked_add(x as usize))
        .ok_or_else(|| AppError::Decode("APNG pixel index overflow".to_string()))?;
    pixel_index
        .checked_mul(4)
        .ok_or_else(|| AppError::Decode("APNG byte index overflow".to_string()))
}

fn blend_over(dst: &mut [u8], src: &[u8]) {
    let src_a = f32::from(src[3]) / 255.0;
    let dst_a = f32::from(dst[3]) / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);

    if out_a <= f32::EPSILON {
        dst.fill(0);
        return;
    }

    for channel in 0..3 {
        let src_c = f32::from(src[channel]) / 255.0;
        let dst_c = f32::from(dst[channel]) / 255.0;
        let out_c = (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
        dst[channel] = (out_c * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Fold duplicate consecutive frames by merging their delays.
/// If two adjacent composited frames are pixel-identical, the second frame
/// is removed and its delay is added to the first.
pub fn fold_duplicate_frames(apng: &mut ApngImage) {
    if apng.frames.len() < 2 {
        return;
    }

    // We need composited frames to compare visual output
    let composited = match compose_frames(apng) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut keep = vec![true; apng.frames.len()];
    let mut i = 0;
    while i < apng.frames.len() {
        if !keep[i] {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < apng.frames.len() && composited[i] == composited[j] {
            // Merge delay from frame j into frame i
            let merged = merge_delays(
                apng.frames[i].delay_num,
                apng.frames[i].delay_den,
                apng.frames[j].delay_num,
                apng.frames[j].delay_den,
            );
            apng.frames[i].delay_num = merged.0;
            apng.frames[i].delay_den = merged.1;
            keep[j] = false;
            j += 1;
        }
        i = j;
    }

    let mut idx = 0;
    apng.frames.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });
}

/// Minimize frame rectangles by computing the minimal bounding box of
/// changed pixels between consecutive composited frames.
pub fn minimize_frame_rects(apng: &mut ApngImage) {
    if apng.frames.len() < 2 {
        return;
    }

    let composited = match compose_frames(apng) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Build a "previous composited" canvas for each frame to diff against
    // Frame 0 diffs against a transparent canvas
    let transparent = vec![0u8; (apng.width as usize) * (apng.height as usize) * 4];

    for i in 1..apng.frames.len() {
        let prev = if i == 0 {
            &transparent
        } else {
            &composited[i - 1]
        };
        let curr = &composited[i];

        // Find the minimal bounding box of changed pixels
        let (min_x, min_y, max_x, max_y) = find_changed_rect(prev, curr, apng.width, apng.height);

        if min_x > max_x || min_y > max_y {
            // No change — make it a 1x1 transparent pixel
            apng.frames[i].x_offset = 0;
            apng.frames[i].y_offset = 0;
            apng.frames[i].width = 1;
            apng.frames[i].height = 1;
            apng.frames[i].rgba = vec![0, 0, 0, 0];
            apng.frames[i].blend_op = BlendOp::Source;
            apng.frames[i].dispose_op = DisposeOp::None;
            continue;
        }

        let new_w = max_x - min_x + 1;
        let new_h = max_y - min_y + 1;

        // Extract the sub-rectangle from the composited frame
        let mut sub_rgba = Vec::with_capacity((new_w * new_h * 4) as usize);
        for y in min_y..=max_y {
            let row_start = ((y * apng.width + min_x) * 4) as usize;
            let row_end = row_start + (new_w * 4) as usize;
            sub_rgba.extend_from_slice(&curr[row_start..row_end]);
        }

        apng.frames[i].x_offset = min_x;
        apng.frames[i].y_offset = min_y;
        apng.frames[i].width = new_w;
        apng.frames[i].height = new_h;
        apng.frames[i].rgba = sub_rgba;
        // Use Source blend + None dispose so the sub-rect overwrites exactly
        apng.frames[i].blend_op = BlendOp::Source;
        apng.frames[i].dispose_op = DisposeOp::None;
    }
}

fn find_changed_rect(prev: &[u8], curr: &[u8], width: u32, height: u32) -> (u32, u32, u32, u32) {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            if prev[idx..idx + 4] != curr[idx..idx + 4] {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    (min_x, min_y, max_x, max_y)
}

fn merge_delays(num1: u16, den1: u16, num2: u16, den2: u16) -> (u16, u16) {
    let d1 = if den1 == 0 { 100 } else { den1 };
    let d2 = if den2 == 0 { 100 } else { den2 };

    if d1 == d2 {
        // Same denominator, just add numerators
        let sum = (num1 as u32) + (num2 as u32);
        if sum <= u16::MAX as u32 {
            return (sum as u16, d1);
        }
    }

    // Cross-multiply for common denominator
    let total_num = (num1 as u32) * (d2 as u32) + (num2 as u32) * (d1 as u32);
    let total_den = (d1 as u32) * (d2 as u32);

    // Try to simplify with GCD
    let g = gcd(total_num, total_den);
    let sn = total_num / g;
    let sd = total_den / g;

    if sn <= u16::MAX as u32 && sd <= u16::MAX as u32 {
        (sn as u16, sd as u16)
    } else {
        // Fallback: convert to milliseconds
        let ms = (total_num * 1000) / total_den;
        let ms = ms.min(u16::MAX as u32);
        (ms as u16, 1000)
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Lightweight input characteristics for APNG optimization decisions.
#[derive(Debug, Clone)]
pub struct ApngInputInfo {
    pub is_indexed: bool,
    pub has_subrect_frames: bool,
}

/// Scan raw PNG bytes to detect palette mode and sub-rect frames without full decode.
pub fn detect_input_characteristics(input: &[u8]) -> ApngInputInfo {
    let mut is_indexed = false;
    let mut has_subrect_frames = false;
    let mut canvas_width: u32 = 0;
    let mut canvas_height: u32 = 0;

    // Parse PNG chunks
    if input.len() < 8 {
        return ApngInputInfo {
            is_indexed: false,
            has_subrect_frames: false,
        };
    }

    let mut pos = 8; // skip PNG signature
    while pos + 12 <= input.len() {
        let chunk_len =
            u32::from_be_bytes([input[pos], input[pos + 1], input[pos + 2], input[pos + 3]])
                as usize;
        let chunk_type = &input[pos + 4..pos + 8];

        match chunk_type {
            b"IHDR" if chunk_len >= 13 => {
                let data = &input[pos + 8..pos + 8 + chunk_len.min(input.len() - pos - 8)];
                if data.len() >= 13 {
                    canvas_width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                    canvas_height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                    // color_type == 3 means indexed
                    is_indexed = data[9] == 3;
                }
            }
            b"PLTE" => {
                is_indexed = true;
            }
            b"fcTL" if chunk_len >= 26 => {
                let data = &input[pos + 8..pos + 8 + chunk_len.min(input.len() - pos - 8)];
                if data.len() >= 26 {
                    let fw = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                    let fh = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
                    let fx = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
                    let fy = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
                    if fw != canvas_width || fh != canvas_height || fx != 0 || fy != 0 {
                        has_subrect_frames = true;
                    }
                }
            }
            b"IEND" => break,
            _ => {}
        }

        // Move to next chunk: 4(len) + 4(type) + data + 4(crc)
        pos += 12 + chunk_len;
    }

    ApngInputInfo {
        is_indexed,
        has_subrect_frames,
    }
}

/// Minimize frame rectangles with post-verification.
/// Returns `true` if optimization was applied and verified, `false` if rolled back.
pub fn minimize_frame_rects_checked(apng: &mut ApngImage) -> bool {
    if apng.frames.len() < 2 {
        return false;
    }

    // Save composited output before optimization
    let composited_before = match compose_frames(apng) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Save original frames for rollback
    let original_frames = apng.frames.clone();

    // Apply optimization
    minimize_frame_rects(apng);

    // Verify composited equivalence
    let composited_after = match compose_frames(apng) {
        Ok(c) => c,
        Err(_) => {
            apng.frames = original_frames;
            return false;
        }
    };

    if composited_before == composited_after {
        true
    } else {
        // Rollback
        apng.frames = original_frames;
        false
    }
}

/// Find the bounding box of non-transparent content in an RGBA buffer.
/// Returns `(content_width, content_height)` — the minimal dimensions
/// that contain all pixels with alpha > 0, anchored at top-left (0,0).
fn find_content_bounds(rgba: &[u8], w: u32, h: u32) -> (u32, u32) {
    let mut max_row: u32 = 0;
    let mut max_col: u32 = 0;
    let mut found = false;

    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4 + 3) as usize;
            if rgba[idx] > 0 {
                if x + 1 > max_col {
                    max_col = x + 1;
                }
                if y + 1 > max_row {
                    max_row = y + 1;
                }
                found = true;
            }
        }
    }

    if !found {
        (w, h) // fully transparent — return original dimensions
    } else {
        (max_col, max_row)
    }
}

/// Conservative frame trim for safe mode.
/// Only trims trailing transparent rows/columns from full-canvas Source frames.
/// Does NOT change dispose_op or blend_op — only shrinks frame dimensions.
pub fn cautious_frame_trim(apng: &mut ApngImage) {
    for frame in &mut apng.frames {
        // Only trim full-canvas Source frames (safe: no semantic change)
        if frame.blend_op != BlendOp::Source {
            continue;
        }
        if frame.width != apng.width || frame.height != apng.height {
            continue;
        }
        if frame.x_offset != 0 || frame.y_offset != 0 {
            continue;
        }

        // Find content bounds
        let (trim_w, trim_h) = find_content_bounds(&frame.rgba, frame.width, frame.height);

        if trim_w == frame.width && trim_h == frame.height {
            continue; // nothing to trim
        }
        if trim_w == 0 || trim_h == 0 {
            continue; // fully transparent — don't collapse to 0x0
        }

        // Extract top-left sub-rect (0,0 to trim_w×trim_h)
        let mut trimmed = Vec::with_capacity((trim_w * trim_h * 4) as usize);
        for y in 0..trim_h {
            let row_start = (y * frame.width * 4) as usize;
            let row_end = row_start + (trim_w * 4) as usize;
            trimmed.extend_from_slice(&frame.rgba[row_start..row_end]);
        }

        frame.width = trim_w;
        frame.height = trim_h;
        frame.rgba = trimmed;
        // x_offset, y_offset stay 0; blend_op, dispose_op unchanged
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApngDefaultImage, ApngFrame, ApngImage, IndexedApngFrame, IndexedApngImage,
        cautious_frame_trim, compose_frames, decode_apng, detect_input_characteristics,
        encode_apng, encode_indexed_apng, find_content_bounds, fold_duplicate_frames, merge_delays,
        minimize_frame_rects, minimize_frame_rects_checked,
    };
    use png::{BlendOp, ColorType, DisposeOp};

    fn rgba(px: &[[u8; 4]]) -> Vec<u8> {
        px.iter().flat_map(|px| px.iter().copied()).collect()
    }

    fn encode_static_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("write header");
        writer.write_image_data(rgba).expect("write image");
        drop(writer);
        out
    }

    fn encode_sample_apng_with_thumbnail() -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 2, 2);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(1, 0).expect("animated");
        encoder
            .set_sep_def_img(true)
            .expect("separate default image");
        let mut writer = encoder.write_header().expect("write header");

        let default_image = rgba(&[[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        writer
            .write_image_data(&default_image)
            .expect("write default image");

        writer
            .set_frame_dimension(1, 1)
            .expect("set frame dimension");
        writer.set_frame_position(1, 0).expect("set frame position");
        writer.set_frame_delay(3, 100).expect("set frame delay");
        writer.set_blend_op(BlendOp::Source).expect("set blend");
        writer.set_dispose_op(DisposeOp::None).expect("set dispose");
        writer
            .write_image_data(&rgba(&[[255, 0, 0, 255]]))
            .expect("write animation frame");
        writer.finish().expect("finish");
        out
    }

    #[test]
    fn static_png_is_not_reported_as_apng() {
        let png = encode_static_png(1, 1, &rgba(&[[1, 2, 3, 255]]));
        assert!(decode_apng(&png).expect("decode static").is_none());
    }

    #[test]
    fn decode_apng_tracks_separate_default_image() {
        let apng = decode_apng(&encode_sample_apng_with_thumbnail())
            .expect("decode apng")
            .expect("is apng");
        assert_eq!(apng.width, 2);
        assert_eq!(apng.height, 2);
        assert!(apng.default_image.is_some());
        assert_eq!(apng.frames.len(), 1);
        let frame = &apng.frames[0];
        assert_eq!(frame.width, 1);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.x_offset, 1);
        assert_eq!(frame.y_offset, 0);
        assert_eq!(frame.delay_num, 3);
        assert_eq!(frame.delay_den, 100);
    }

    #[test]
    fn compose_frames_respects_blend_and_dispose_previous() {
        let apng = ApngImage {
            width: 2,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 30,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255], [255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 30,
                    dispose_op: DisposeOp::Previous,
                    blend_op: BlendOp::Over,
                    rgba: rgba(&[[0, 0, 255, 128]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 1,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 30,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
            ],
        };

        let composited = compose_frames(&apng).expect("compose");
        assert_eq!(composited.len(), 3);
        assert_eq!(composited[0], rgba(&[[255, 0, 0, 255], [255, 0, 0, 255]]));
        assert_eq!(composited[1], rgba(&[[127, 0, 128, 255], [255, 0, 0, 255]]));
        assert_eq!(composited[2], rgba(&[[255, 0, 0, 255], [0, 255, 0, 255]]));
    }

    #[test]
    fn encode_decode_round_trip_preserves_composited_outputs() {
        let original = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: Some(ApngDefaultImage {
                rgba: rgba(&[[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]),
            }),
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 1,
                    y_offset: 1,
                    delay_num: 2,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
            ],
        };

        let encoded = encode_apng(&original).expect("encode apng");
        let decoded = decode_apng(&encoded)
            .expect("decode apng")
            .expect("is apng");
        assert!(decoded.default_image.is_some());
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(
            compose_frames(&decoded).expect("compose decoded"),
            compose_frames(&original).expect("compose original")
        );
    }

    #[test]
    fn fold_duplicate_frames_merges_identical_consecutive() {
        let mut apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 2,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 3,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
            ],
        };
        fold_duplicate_frames(&mut apng);
        assert_eq!(apng.frames.len(), 2);
        assert_eq!(apng.frames[0].delay_num, 3);
        assert_eq!(apng.frames[0].delay_den, 10);
        assert_eq!(apng.frames[1].rgba, rgba(&[[0, 255, 0, 255]]));
    }

    #[test]
    fn fold_duplicate_frames_no_duplicates_is_noop() {
        let mut apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
            ],
        };
        fold_duplicate_frames(&mut apng);
        assert_eq!(apng.frames.len(), 2);
    }

    #[test]
    fn minimize_frame_rects_shrinks_unchanged_regions() {
        let mut apng = ApngImage {
            width: 3,
            height: 3,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                // Frame 2: only pixel (2,2) changes
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 255, 0, 255],
                    ]),
                },
            ],
        };

        let composited_before = compose_frames(&apng).expect("compose before");
        minimize_frame_rects(&mut apng);

        // Frame 1 should be minimized to 1x1 at offset (2,2)
        assert_eq!(apng.frames[1].width, 1);
        assert_eq!(apng.frames[1].height, 1);
        assert_eq!(apng.frames[1].x_offset, 2);
        assert_eq!(apng.frames[1].y_offset, 2);
        assert_eq!(apng.frames[1].rgba, rgba(&[[0, 255, 0, 255]]));

        // Composited output should be identical
        let composited_after = compose_frames(&apng).expect("compose after");
        assert_eq!(composited_before, composited_after);
    }

    #[test]
    fn merge_delays_same_denominator() {
        assert_eq!(merge_delays(1, 10, 2, 10), (3, 10));
    }

    #[test]
    fn merge_delays_different_denominator() {
        // 1/10 + 1/20 = 3/20
        assert_eq!(merge_delays(1, 10, 1, 20), (3, 20));
    }

    #[test]
    fn apng_pipeline_round_trip_preserves_animation() {
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        let original = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [0, 0, 0, 255],
                        [0, 255, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
            ],
        };
        let composited_orig = compose_frames(&original).expect("compose original");
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions::default();

        let result = process_png_bytes(&encoded, options).expect("pipeline");
        assert_eq!(result.quality_score, 100);
        assert_eq!(result.quality_mse, 0.0);

        // Verify the output is still a valid APNG with same composited frames
        let decoded = decode_apng(&result.png_data)
            .expect("decode output")
            .expect("is apng");
        let composited_out = compose_frames(&decoded).expect("compose output");
        assert_eq!(composited_orig, composited_out);
    }

    // ── compose_frames tests ──

    #[test]
    fn compose_dispose_background_clears_region() {
        let apng = ApngImage {
            width: 2,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::Background,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255], [0, 255, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 0, 255, 255]]),
                },
            ],
        };

        let composited = compose_frames(&apng).expect("compose");
        // After frame 0 dispose=background, canvas should be cleared before frame 1
        // Frame 1 writes blue at (0,0), pixel (1,0) should be transparent (cleared)
        assert_eq!(composited[1], rgba(&[[0, 0, 255, 255], [0, 0, 0, 0]]));
    }

    #[test]
    fn compose_blend_over_partial_alpha() {
        let apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Over,
                    // 50% transparent green over opaque red
                    rgba: rgba(&[[0, 255, 0, 128]]),
                },
            ],
        };
        let composited = compose_frames(&apng).expect("compose");
        // Alpha blending: green over red
        let result = &composited[1];
        // Result should be opaque with blended RGB
        assert_eq!(result[3], 255); // fully opaque output
        assert!(result[0] < 200); // red reduced
        assert!(result[1] > 50); // green present
    }

    #[test]
    fn compose_dispose_previous_first_frame_degrades_to_background() {
        // Per APNG spec: DisposeOp::Previous on frame 0 degrades to Background
        let apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::Previous, // degrades to Background for frame 0
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
            ],
        };
        let composited = compose_frames(&apng).expect("compose");
        assert_eq!(composited[0], rgba(&[[255, 0, 0, 255]]));
        // After frame 0 with Previous->Background, canvas is cleared
        assert_eq!(composited[1], rgba(&[[0, 255, 0, 255]]));
    }

    // ── minimize_frame_rects tests ──

    #[test]
    fn minimize_already_subrect_input() {
        // Input already has sub-rect frames, minimize should still produce correct composites
        let mut apng = ApngImage {
            width: 3,
            height: 3,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![0; 3 * 3 * 4],
                },
                // Already a sub-rect frame
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 1,
                    y_offset: 1,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 255, 0, 255],
                        [0, 0, 255, 255],
                        [255, 255, 0, 255],
                    ]),
                },
            ],
        };

        let composited_before = compose_frames(&apng).expect("compose before");
        minimize_frame_rects(&mut apng);
        let composited_after = compose_frames(&apng).expect("compose after");
        assert_eq!(composited_before, composited_after);
    }

    #[test]
    fn minimize_no_change_frame_becomes_1x1() {
        let frame_rgba = rgba(&[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ]);
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: frame_rgba.clone(),
                },
                // Identical to frame 0
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: frame_rgba,
                },
            ],
        };

        minimize_frame_rects(&mut apng);
        // No-change frame should become 1x1 transparent
        assert_eq!(apng.frames[1].width, 1);
        assert_eq!(apng.frames[1].height, 1);
        assert_eq!(apng.frames[1].rgba, vec![0, 0, 0, 0]);
    }

    #[test]
    fn minimize_full_change_stays_full() {
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [255, 0, 0, 255],
                        [255, 0, 0, 255],
                        [255, 0, 0, 255],
                    ]),
                },
                // Every pixel changes
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [0, 255, 0, 255],
                        [0, 255, 0, 255],
                        [0, 255, 0, 255],
                        [0, 255, 0, 255],
                    ]),
                },
            ],
        };

        minimize_frame_rects(&mut apng);
        // Full change: should stay 2x2 at (0,0)
        assert_eq!(apng.frames[1].width, 2);
        assert_eq!(apng.frames[1].height, 2);
        assert_eq!(apng.frames[1].x_offset, 0);
        assert_eq!(apng.frames[1].y_offset, 0);
    }

    #[test]
    fn minimize_single_frame_noop() {
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 2,
                height: 2,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: rgba(&[
                    [255, 0, 0, 255],
                    [0, 255, 0, 255],
                    [0, 0, 255, 255],
                    [255, 255, 0, 255],
                ]),
            }],
        };
        let original = apng.clone();
        minimize_frame_rects(&mut apng);
        assert_eq!(apng.frames[0].width, original.frames[0].width);
        assert_eq!(apng.frames[0].height, original.frames[0].height);
    }

    #[test]
    fn minimize_preserves_composited_equivalence() {
        // Multi-frame with various changes — core property test
        let mut apng = ApngImage {
            width: 4,
            height: 4,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 4,
                    height: 4,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![128; 4 * 4 * 4],
                },
                ApngFrame {
                    width: 4,
                    height: 4,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: {
                        let mut d = vec![128; 4 * 4 * 4];
                        // Change only pixels (1,1) and (2,3)
                        let idx1 = (1 * 4 + 1) * 4;
                        d[idx1..idx1 + 4].copy_from_slice(&[255, 0, 0, 255]);
                        let idx2 = (3 * 4 + 2) * 4;
                        d[idx2..idx2 + 4].copy_from_slice(&[0, 255, 0, 255]);
                        d
                    },
                },
                ApngFrame {
                    width: 4,
                    height: 4,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![64; 4 * 4 * 4],
                },
            ],
        };

        let composited_before = compose_frames(&apng).expect("compose before");
        minimize_frame_rects(&mut apng);
        let composited_after = compose_frames(&apng).expect("compose after");
        assert_eq!(composited_before, composited_after);
    }

    // ── minimize_frame_rects_checked tests ──

    #[test]
    fn minimize_checked_succeeds_on_valid_input() {
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 255, 0, 255],
                    ]),
                },
            ],
        };
        assert!(minimize_frame_rects_checked(&mut apng));
        // Should have been minimized
        assert!(apng.frames[1].width < 2 || apng.frames[1].height < 2);
    }

    #[test]
    fn minimize_checked_single_frame_returns_false() {
        let mut apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 1,
                height: 1,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: rgba(&[[255, 0, 0, 255]]),
            }],
        };
        assert!(!minimize_frame_rects_checked(&mut apng));
    }

    // ── fold_duplicate_frames additional tests ──

    #[test]
    fn fold_non_consecutive_duplicates() {
        // A, A, B, B -> A, B
        let mut apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[0, 255, 0, 255]]),
                },
            ],
        };
        fold_duplicate_frames(&mut apng);
        assert_eq!(apng.frames.len(), 2);
        assert_eq!(apng.frames[0].delay_num, 2);
        assert_eq!(apng.frames[1].delay_num, 2);
    }

    #[test]
    fn fold_all_identical() {
        let mut apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
            ],
        };
        fold_duplicate_frames(&mut apng);
        assert_eq!(apng.frames.len(), 1);
        assert_eq!(apng.frames[0].delay_num, 3);
        assert_eq!(apng.frames[0].delay_den, 10);
    }

    #[test]
    fn fold_delay_overflow_uses_millisecond_fallback() {
        let mut apng = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 60000,
                    delay_den: 1,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
                ApngFrame {
                    width: 1,
                    height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 60000,
                    delay_den: 1,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[[255, 0, 0, 255]]),
                },
            ],
        };
        fold_duplicate_frames(&mut apng);
        assert_eq!(apng.frames.len(), 1);
        // Should have used millisecond fallback since 120000 > u16::MAX
        assert_eq!(apng.frames[0].delay_den, 1000);
    }

    // ── detect_input_characteristics tests ──

    #[test]
    fn detect_rgba_apng_not_indexed() {
        let apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![255; 2 * 2 * 4],
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![128; 2 * 2 * 4],
                },
            ],
        };
        let encoded = encode_apng(&apng).expect("encode");
        let info = detect_input_characteristics(&encoded);
        assert!(!info.is_indexed);
        assert!(!info.has_subrect_frames);
    }

    #[test]
    fn detect_subrect_frames() {
        let apng = ApngImage {
            width: 4,
            height: 4,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 4,
                    height: 4,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![0; 4 * 4 * 4],
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 1,
                    y_offset: 1,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![255; 2 * 2 * 4],
                },
            ],
        };
        let encoded = encode_apng(&apng).expect("encode");
        let info = detect_input_characteristics(&encoded);
        assert!(!info.is_indexed); // RGBA encoded
        assert!(info.has_subrect_frames);
    }

    #[test]
    fn detect_static_indexed_png() {
        let info = detect_input_characteristics(&encode_static_png(1, 1, &rgba(&[[1, 2, 3, 255]])));
        // Static PNG encoded as RGBA by encode_static_png
        assert!(!info.is_indexed);
    }

    // ── Pipeline integration tests (Step 4) ──

    #[test]
    fn pipeline_apng_safe_mode_preserves_frames() {
        use crate::cli::ApngMode;
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        let original = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [0, 0, 0, 255],
                        [0, 255, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
            ],
        };
        let composited_orig = compose_frames(&original).expect("compose original");
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions {
            apng_mode: ApngMode::Safe,
            ..PipelineOptions::default()
        };
        let result = process_png_bytes(&encoded, options).expect("pipeline safe");

        let decoded = decode_apng(&result.png_data)
            .expect("decode output")
            .expect("is apng");
        let composited_out = compose_frames(&decoded).expect("compose output");
        assert_eq!(composited_orig, composited_out);
    }

    #[test]
    fn pipeline_apng_aggressive_mode_preserves_frames() {
        use crate::cli::ApngMode;
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        let original = ApngImage {
            width: 3,
            height: 3,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![128; 3 * 3 * 4],
                },
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: {
                        let mut d = vec![128; 3 * 3 * 4];
                        // Change pixel (2,2)
                        let idx = (2 * 3 + 2) * 4;
                        d[idx..idx + 4].copy_from_slice(&[0, 255, 0, 255]);
                        d
                    },
                },
            ],
        };
        let composited_orig = compose_frames(&original).expect("compose original");
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions {
            apng_mode: ApngMode::Aggressive,
            ..PipelineOptions::default()
        };
        let result = process_png_bytes(&encoded, options).expect("pipeline aggressive");

        let decoded = decode_apng(&result.png_data)
            .expect("decode output")
            .expect("is apng");
        let composited_out = compose_frames(&decoded).expect("compose output");
        assert_eq!(composited_orig, composited_out);
    }

    #[test]
    fn pipeline_apng_skip_if_larger() {
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        // Create a tiny APNG that will likely grow after re-encoding
        let original = ApngImage {
            width: 1,
            height: 1,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 1,
                height: 1,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: rgba(&[[255, 0, 0, 255]]),
            }],
        };
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions {
            skip_if_larger: true,
            ..PipelineOptions::default()
        };
        // This may or may not trigger SkipIfLargerRejected depending on sizes
        let _ = process_png_bytes(&encoded, options);
    }

    #[test]
    fn pipeline_apng_fold_reduces_frame_count() {
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        let original = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                // Duplicate frame
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [0, 255, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
            ],
        };
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions::default();
        let result = process_png_bytes(&encoded, options).expect("pipeline");

        let decoded = decode_apng(&result.png_data)
            .expect("decode output")
            .expect("is apng");
        // Should have folded duplicate: 3 -> 2 frames
        assert_eq!(decoded.frames.len(), 2);
    }

    #[test]
    fn pipeline_apng_safe_mode_no_subrect() {
        use crate::cli::ApngMode;
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        let original = ApngImage {
            width: 3,
            height: 3,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: vec![128; 3 * 3 * 4],
                },
                ApngFrame {
                    width: 3,
                    height: 3,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: {
                        let mut d = vec![128; 3 * 3 * 4];
                        d[0..4].copy_from_slice(&[0, 255, 0, 255]);
                        d
                    },
                },
            ],
        };
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions {
            apng_mode: ApngMode::Safe,
            ..PipelineOptions::default()
        };
        let result = process_png_bytes(&encoded, options).expect("pipeline safe");

        let decoded = decode_apng(&result.png_data)
            .expect("decode output")
            .expect("is apng");
        // Safe mode should NOT produce sub-rect frames
        for frame in &decoded.frames {
            assert_eq!(frame.width, 3);
            assert_eq!(frame.height, 3);
            assert_eq!(frame.x_offset, 0);
            assert_eq!(frame.y_offset, 0);
        }
    }

    #[test]
    fn pipeline_apng_lossy_quantization_produces_indexed_output() {
        use crate::cli::{ApngMode, QualityRange};
        use crate::pipeline::{PipelineOptions, process_png_bytes};

        // Create a multi-color APNG that will actually get quantized
        let mut frame1_rgba = Vec::with_capacity(8 * 8 * 4);
        let mut frame2_rgba = Vec::with_capacity(8 * 8 * 4);
        for y in 0..8u8 {
            for x in 0..8u8 {
                frame1_rgba.extend_from_slice(&[x * 32, y * 32, 128, 255]);
                frame2_rgba.extend_from_slice(&[128, x * 32, y * 32, 255]);
            }
        }

        let original = ApngImage {
            width: 8,
            height: 8,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 8,
                    height: 8,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: frame1_rgba,
                },
                ApngFrame {
                    width: 8,
                    height: 8,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: frame2_rgba,
                },
            ],
        };
        let encoded = encode_apng(&original).expect("encode");

        let options = PipelineOptions {
            quality: Some(QualityRange { raw: "0-80".to_string(), min: 0, max: 80 }),
            apng_mode: ApngMode::Safe,
            ..PipelineOptions::default()
        };
        let result = process_png_bytes(&encoded, options).expect("pipeline lossy APNG");

        // Lossy quantization should produce a valid quality_score
        assert!(
            result.quality_score <= 100,
            "quality_score={} should be <= 100",
            result.quality_score
        );
        // quality_mse should be populated (non-negative)
        assert!(
            result.quality_mse >= 0.0,
            "quality_mse should be non-negative"
        );

        // Verify it's still a valid APNG with 2 frames
        let decoded = decode_apng(&result.png_data)
            .expect("decode output")
            .expect("is apng");
        assert_eq!(decoded.frames.len(), 2);
    }

    // ── Round-trip equivalence for encode/decode ──

    #[test]
    fn round_trip_dispose_none_source() {
        round_trip_verify(&make_apng_2x2(DisposeOp::None, BlendOp::Source));
    }

    #[test]
    fn round_trip_dispose_bg_source() {
        round_trip_verify(&make_apng_2x2(DisposeOp::Background, BlendOp::Source));
    }

    #[test]
    fn round_trip_dispose_prev_source() {
        round_trip_verify(&make_apng_3frame_prev(BlendOp::Source));
    }

    fn round_trip_verify(original: &ApngImage) {
        let composited_orig = compose_frames(original).expect("compose original");
        let encoded = encode_apng(original).expect("encode");
        let decoded = decode_apng(&encoded).expect("decode").expect("is apng");
        let composited_decoded = compose_frames(&decoded).expect("compose decoded");
        assert_eq!(composited_orig, composited_decoded);
    }

    fn make_apng_2x2(dispose: DisposeOp, blend: BlendOp) -> ApngImage {
        ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: dispose,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: blend,
                    rgba: rgba(&[
                        [0, 0, 0, 255],
                        [0, 255, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 255, 255],
                    ]),
                },
            ],
        }
    }

    fn make_apng_3frame_prev(blend: BlendOp) -> ApngImage {
        ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                        [0, 0, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::Previous,
                    blend_op: blend,
                    rgba: rgba(&[
                        [0, 255, 0, 255],
                        [0, 255, 0, 255],
                        [0, 255, 0, 255],
                        [0, 255, 0, 255],
                    ]),
                },
                ApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: blend,
                    rgba: rgba(&[
                        [0, 0, 255, 255],
                        [0, 0, 255, 255],
                        [0, 0, 255, 255],
                        [0, 0, 255, 255],
                    ]),
                },
            ],
        }
    }

    // --- cautious_frame_trim tests ---

    #[test]
    fn cautious_trim_removes_trailing_transparent_rows() {
        // 4x4 frame, bottom 2 rows fully transparent -> 4x2
        let mut apng = ApngImage {
            width: 4,
            height: 4,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 4,
                height: 4,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: {
                    let mut d = vec![];
                    // Row 0-1: opaque red
                    for _ in 0..8 {
                        d.extend_from_slice(&[255, 0, 0, 255]);
                    }
                    // Row 2-3: transparent
                    for _ in 0..8 {
                        d.extend_from_slice(&[0, 0, 0, 0]);
                    }
                    d
                },
            }],
        };
        cautious_frame_trim(&mut apng);
        assert_eq!(apng.frames[0].width, 4);
        assert_eq!(apng.frames[0].height, 2);
        assert_eq!(apng.frames[0].rgba.len(), 4 * 2 * 4);
    }

    #[test]
    fn cautious_trim_removes_trailing_transparent_cols() {
        // 4x4 frame, right 2 columns fully transparent -> 2x4
        let mut apng = ApngImage {
            width: 4,
            height: 4,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 4,
                height: 4,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: {
                    let mut d = vec![];
                    for _ in 0..4 {
                        // 2 opaque + 2 transparent per row
                        d.extend_from_slice(&[255, 0, 0, 255]);
                        d.extend_from_slice(&[0, 255, 0, 255]);
                        d.extend_from_slice(&[0, 0, 0, 0]);
                        d.extend_from_slice(&[0, 0, 0, 0]);
                    }
                    d
                },
            }],
        };
        cautious_frame_trim(&mut apng);
        assert_eq!(apng.frames[0].width, 2);
        assert_eq!(apng.frames[0].height, 4);
        assert_eq!(apng.frames[0].rgba.len(), 2 * 4 * 4);
    }

    #[test]
    fn cautious_trim_skips_over_blend_frames() {
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 2,
                height: 2,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Over, // Over, not Source
                rgba: rgba(&[[255, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]),
            }],
        };
        let original_len = apng.frames[0].rgba.len();
        cautious_frame_trim(&mut apng);
        assert_eq!(apng.frames[0].width, 2);
        assert_eq!(apng.frames[0].height, 2);
        assert_eq!(apng.frames[0].rgba.len(), original_len);
    }

    #[test]
    fn cautious_trim_skips_subrect_frames() {
        let mut apng = ApngImage {
            width: 4,
            height: 4,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 2, // sub-rect, not full canvas
                height: 2,
                x_offset: 1,
                y_offset: 1,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: rgba(&[[255, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]),
            }],
        };
        cautious_frame_trim(&mut apng);
        assert_eq!(apng.frames[0].width, 2);
        assert_eq!(apng.frames[0].height, 2);
    }

    #[test]
    fn cautious_trim_preserves_composited_equivalence() {
        // 2-frame APNG: frame 0 has trailing transparent region
        let mut apng = ApngImage {
            width: 4,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![
                ApngFrame {
                    width: 4,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [255, 0, 0, 255],
                        [0, 255, 0, 255],
                        [0, 0, 0, 0],
                        [0, 0, 0, 0],
                        [0, 0, 255, 255],
                        [255, 255, 0, 255],
                        [0, 0, 0, 0],
                        [0, 0, 0, 0],
                    ]),
                },
                ApngFrame {
                    width: 4,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    rgba: rgba(&[
                        [128, 128, 128, 255],
                        [64, 64, 64, 255],
                        [32, 32, 32, 255],
                        [16, 16, 16, 255],
                        [128, 128, 128, 255],
                        [64, 64, 64, 255],
                        [32, 32, 32, 255],
                        [16, 16, 16, 255],
                    ]),
                },
            ],
        };

        let composited_before = compose_frames(&apng).unwrap();
        cautious_frame_trim(&mut apng);
        let composited_after = compose_frames(&apng).unwrap();

        assert_eq!(composited_before, composited_after);
        // First frame should be trimmed (right 2 cols transparent)
        assert_eq!(apng.frames[0].width, 2);
        // Second frame is fully opaque, no trim
        assert_eq!(apng.frames[1].width, 4);
    }

    #[test]
    fn cautious_trim_noop_when_no_transparent_border() {
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 2,
                height: 2,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: rgba(&[
                    [255, 0, 0, 255],
                    [0, 255, 0, 255],
                    [0, 0, 255, 255],
                    [255, 255, 0, 255],
                ]),
            }],
        };
        cautious_frame_trim(&mut apng);
        assert_eq!(apng.frames[0].width, 2);
        assert_eq!(apng.frames[0].height, 2);
    }

    #[test]
    fn cautious_trim_fully_transparent_frame_unchanged() {
        let mut apng = ApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            default_image: None,
            frames: vec![ApngFrame {
                width: 2,
                height: 2,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 10,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
                rgba: rgba(&[[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]),
            }],
        };
        cautious_frame_trim(&mut apng);
        // Fully transparent — should NOT collapse to 0x0
        assert_eq!(apng.frames[0].width, 2);
        assert_eq!(apng.frames[0].height, 2);
    }

    #[test]
    fn find_content_bounds_basic() {
        // 3x3 image, content in top-left 2x2
        let data = rgba(&[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 0, 0],
            [0, 0, 255, 255],
            [255, 255, 0, 128],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        ]);
        let (w, h) = find_content_bounds(&data, 3, 3);
        assert_eq!((w, h), (2, 2));
    }

    #[test]
    fn encode_indexed_apng_round_trip() {
        // Create a simple 2-frame indexed APNG and verify it decodes back
        let palette = vec![
            [255, 0, 0, 255],   // red
            [0, 255, 0, 255],   // green
            [0, 0, 255, 255],   // blue
            [0, 0, 0, 0],       // transparent
        ];
        let indexed = IndexedApngImage {
            width: 2,
            height: 2,
            num_plays: 0,
            palette: palette.clone(),
            default_image_indices: None,
            frames: vec![
                IndexedApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    indices: vec![0, 1, 2, 3], // red, green, blue, transparent
                },
                IndexedApngFrame {
                    width: 2,
                    height: 2,
                    x_offset: 0,
                    y_offset: 0,
                    delay_num: 1,
                    delay_den: 10,
                    dispose_op: DisposeOp::None,
                    blend_op: BlendOp::Source,
                    indices: vec![1, 2, 0, 3], // green, blue, red, transparent
                },
            ],
        };

        let encoded = encode_indexed_apng(&indexed).expect("encode indexed APNG");

        // Verify it decodes as APNG
        let decoded = decode_apng(&encoded).expect("decode").expect("should be APNG");
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);

        // Verify composited frame 0 matches expected RGBA
        let composited = compose_frames(&decoded).expect("compose");
        // Frame 0: red, green, blue, transparent (expanded to RGBA)
        assert_eq!(&composited[0][0..4], &[255, 0, 0, 255]);
        assert_eq!(&composited[0][4..8], &[0, 255, 0, 255]);
        assert_eq!(&composited[0][8..12], &[0, 0, 255, 255]);
        assert_eq!(&composited[0][12..16], &[0, 0, 0, 0]);
    }
}
