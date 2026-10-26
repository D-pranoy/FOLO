//! The CPU downscale of the shared-memory path (ADR-0037 D17): the buffer
//! PipeWire hands over becomes the small sRGB rgb8 picture of the frame, with
//! the area-weighted mean in linear light that `folo_capture_core::color`
//! defines. This is the reference's arithmetic, laid out to run once per frame
//! without building the whole picture in linear light: each input row is
//! reduced across, then added into the output rows it overlaps.

use std::thread;

use folo_capture_core::color::srgb_to_linear;
use folo_capture_core::error::CaptureError;

/// A picture of at least this many input pixels is scaled by several threads.
const LARGE_PICTURE_PIXELS: usize = 1_000_000;
/// The most threads that scale one picture.
const MOST_BANDS: usize = 4;

/// How a buffer lays out a pixel in memory, by byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelLayout {
    /// `B G R x` (SPA `BGRx`, what most compositors send).
    Bgrx,
    /// `B G R A`; alpha is ignored, a screen is opaque.
    Bgra,
    Rgbx,
    Rgba,
    Rgb,
    Bgr,
}

impl PixelLayout {
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgrx | Self::Bgra | Self::Rgbx | Self::Rgba => 4,
            Self::Rgb | Self::Bgr => 3,
        }
    }

    /// The offsets of the red, green and blue bytes in a pixel.
    const fn offsets(self) -> [usize; 3] {
        match self {
            Self::Bgrx | Self::Bgra | Self::Bgr => [2, 1, 0],
            Self::Rgbx | Self::Rgba | Self::Rgb => [0, 1, 2],
        }
    }
}

/// A buffer as PipeWire describes it.
#[derive(Clone, Copy)]
pub struct SourceImage<'a> {
    pub layout: PixelLayout,
    pub width: usize,
    pub height: usize,
    /// Bytes from one row to the next, at least `width × bytes_per_pixel`.
    pub stride: usize,
    pub data: &'a [u8],
}

// The sizes only: a stray `{:?}` must not print the screen's pixels.
impl std::fmt::Debug for SourceImage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceImage")
            .field("layout", &self.layout)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .finish_non_exhaustive()
    }
}

/// For each output cell along one axis, the input cells under it and how much
/// of each, as shares of the output cell's span. These are the weights of the
/// core reference (`color::downscale`).
#[derive(Debug)]
struct AxisPlan {
    starts: Vec<usize>,
    entries: Vec<(usize, f32)>,
}

impl AxisPlan {
    fn new(input: usize, output: usize) -> Self {
        // Sizes are a screen's: far below 2^53, so the conversions are exact.
        #[allow(clippy::cast_precision_loss)]
        let scale = input as f64 / output as f64;
        let mut starts = Vec::with_capacity(output + 1);
        let mut entries = Vec::new();
        for cell in 0..output {
            starts.push(entries.len());
            #[allow(clippy::cast_precision_loss)]
            let (from, to) = (cell as f64 * scale, (cell + 1) as f64 * scale);
            // The cast is of a non-negative value, and `input` bounds it.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (first, last) = (from.floor() as usize, (to.ceil() as usize).min(input));
            for source in first..last {
                #[allow(clippy::cast_precision_loss)]
                let overlap = to.min((source + 1) as f64) - from.max(source as f64);
                if overlap > 0.0 {
                    // f32 carries a share to 7 digits: far below a level of 255.
                    #[allow(clippy::cast_possible_truncation)]
                    entries.push((source, (overlap / scale) as f32));
                }
            }
        }
        starts.push(entries.len());
        Self { starts, entries }
    }

    /// The input cells under output cell `cell`, with their shares.
    fn cells(&self, cell: usize) -> &[(usize, f32)] {
        &self.entries[self.starts[cell]..self.starts[cell + 1]]
    }
}

/// Downscales buffers of one size to pictures of one size. It keeps the
/// weights and the work arrays, so one is made per (input, output) size pair
/// and reused for every frame.
///
/// Each output row is made from the input rows under it: they are added up
/// column by column in linear light, weighted by their share, and the sums of
/// the columns under each output cell are then added across. That is the
/// separable mean of the reference, in the order that reads each input pixel
/// once (twice on the rows two output rows share).
#[derive(Debug)]
pub struct Downscaler {
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
    across: AxisPlan,
    down: AxisPlan,
    linear: [f32; 256],
    /// For each level k of 0 to 254, the linear light at which the encoded
    /// value passes k + ½: the light encodes to the number of these it reaches.
    thresholds: [f32; 255],
    /// The column sums of each band of output rows, which are scaled by one
    /// thread each when the picture is large.
    columns: Vec<Vec<f32>>,
}

impl Downscaler {
    /// A downscaler from `in_width × in_height` to `out_width × out_height`.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Failed`] for a size of 0.
    pub fn new(
        in_width: usize,
        in_height: usize,
        out_width: usize,
        out_height: usize,
    ) -> Result<Self, CaptureError> {
        if [in_width, in_height, out_width, out_height].contains(&0) {
            return Err(CaptureError::Failed(
                "a frame of size 0 can't be downscaled".to_owned(),
            ));
        }
        let mut linear = [0.0; 256];
        for (value, slot) in (0..=u8::MAX).zip(linear.iter_mut()) {
            // A level's linear light is in 0 to 1, which f32 holds to 7 digits.
            #[allow(clippy::cast_possible_truncation)]
            let light = srgb_to_linear(value) as f32;
            *slot = light;
        }
        Ok(Self {
            in_width,
            in_height,
            out_width,
            out_height,
            across: AxisPlan::new(in_width, out_width),
            down: AxisPlan::new(in_height, out_height),
            linear,
            thresholds: encode_thresholds(),
            columns: vec![vec![0.0; in_width * 3]; bands_for(in_width * in_height, out_height)],
        })
    }

    /// Whether this downscaler is for these sizes.
    #[must_use]
    pub fn is_for(&self, input: (usize, usize), output: (usize, usize)) -> bool {
        (self.in_width, self.in_height) == input && (self.out_width, self.out_height) == output
    }

    /// `out_width × out_height × 3` bytes of sRGB rgb8.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Failed`] when the image isn't this downscaler's input
    /// size, or its stride or data are too short for it.
    pub fn scale(&mut self, image: &SourceImage<'_>) -> Result<Vec<u8>, CaptureError> {
        self.check(image)?;
        let mut picture = vec![0; self.out_width * self.out_height * 3];
        // The band's sums are lent to the bands while the weights are shared.
        let mut sums = std::mem::take(&mut self.columns);
        let band_count = sums.len();
        let rows_per_band = self.out_height.div_ceil(band_count);
        let band = Band {
            scaler: self,
            image,
        };
        let bands = picture
            .chunks_mut(rows_per_band * self.out_width * 3)
            .zip(sums.iter_mut().map(Vec::as_mut_slice))
            .enumerate();
        if band_count == 1 {
            for (index, (rows, columns)) in bands {
                band.scale(index * rows_per_band, columns, rows);
            }
        } else {
            thread::scope(|scope| {
                for (index, (rows, columns)) in bands {
                    let band = &band;
                    scope.spawn(move || band.scale(index * rows_per_band, columns, rows));
                }
            });
        }
        self.columns = sums;
        Ok(picture)
    }

    /// The sRGB byte of a linear light, rounded to nearest as the reference
    /// does, without the power function: a search among the thresholds.
    fn encode(&self, light: f32) -> u8 {
        // At most 255 thresholds, so the count fits a byte.
        u8::try_from(
            self.thresholds
                .partition_point(|threshold| *threshold <= light),
        )
        .unwrap_or(u8::MAX)
    }

    fn check(&self, image: &SourceImage<'_>) -> Result<(), CaptureError> {
        let bytes = image.layout.bytes_per_pixel();
        let wrong = |why: &str| Err(CaptureError::Failed(why.to_owned()));
        if (image.width, image.height) != (self.in_width, self.in_height) {
            return wrong("the buffer isn't the size this downscaler was made for");
        }
        if image.stride < image.width * bytes {
            return wrong("the buffer's stride is shorter than its rows");
        }
        if image.data.len() < (image.height - 1) * image.stride + image.width * bytes {
            return wrong("the buffer is shorter than its size and stride say");
        }
        Ok(())
    }
}

/// How many threads scale one picture: one for a small picture, up to
/// [`MOST_BANDS`] for a large one, never more than it has output rows.
fn bands_for(input_pixels: usize, out_height: usize) -> usize {
    if input_pixels < LARGE_PICTURE_PIXELS {
        return 1;
    }
    thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .clamp(1, MOST_BANDS)
        .min(out_height)
}

/// A downscaler and the image it works on, for one thread's rows.
struct Band<'a> {
    scaler: &'a Downscaler,
    image: &'a SourceImage<'a>,
}

impl Band<'_> {
    /// Scales the output rows from `first_row` that fit `rows` into it.
    fn scale(&self, first_row: usize, columns: &mut [f32], rows: &mut [u8]) {
        let scaler = self.scaler;
        let line_bytes = self.image.width * self.image.layout.bytes_per_pixel();
        for (offset, row) in rows.chunks_exact_mut(scaler.out_width * 3).enumerate() {
            columns.fill(0.0);
            for &(source, share) in scaler.down.cells(first_row + offset) {
                let line = &self.image.data[source * self.image.stride..][..line_bytes];
                accumulate(columns, line, self.image.layout, &scaler.linear, share);
            }
            for (cell, rgb) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let mut sum = [0.0_f32; 3];
                for &(column, share) in scaler.across.cells(cell) {
                    let at = column * 3;
                    sum[0] += columns[at] * share;
                    sum[1] += columns[at + 1] * share;
                    sum[2] += columns[at + 2] * share;
                }
                for (byte, light) in rgb.iter_mut().zip(sum) {
                    *byte = scaler.encode(light);
                }
            }
        }
    }
}

/// The linear light of an encoded value between 0 and 1.
fn decode(encoded: f64) -> f64 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn encode_thresholds() -> [f32; 255] {
    let mut thresholds = [0.0; 255];
    for (level, slot) in (0..255_u8).zip(thresholds.iter_mut()) {
        // f32 holds the light to 7 digits, far finer than the gaps between levels.
        #[allow(clippy::cast_possible_truncation)]
        let light = decode((f64::from(level) + 0.5) / 255.0) as f32;
        *slot = light;
    }
    thresholds
}

/// Adds one input row's linear light, times `share`, to the column sums.
fn accumulate(
    columns: &mut [f32],
    line: &[u8],
    layout: PixelLayout,
    linear: &[f32; 256],
    share: f32,
) {
    let bytes = layout.bytes_per_pixel();
    let [red, green, blue] = layout.offsets();
    // Each level's light times the share, so the inner loop is a load and an add.
    let mut weighted = [0.0_f32; 256];
    for (slot, light) in weighted.iter_mut().zip(linear) {
        *slot = light * share;
    }
    for (pixel, sums) in line
        .chunks_exact(bytes)
        .zip(columns.as_chunks_mut::<3>().0.iter_mut())
    {
        sums[0] += weighted[usize::from(pixel[red])];
        sums[1] += weighted[usize::from(pixel[green])];
        sums[2] += weighted[usize::from(pixel[blue])];
    }
}
