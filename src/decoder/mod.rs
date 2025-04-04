use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::io::{self, Read, Seek};
use std::{cmp, ops::Range};

use crate::{
    bytecast, ColorType, TiffError, TiffFormatError, TiffResult, TiffUnsupportedError, UsageError,
};

use self::ifd::Directory;
use self::image::Image;
use crate::tags::{
    CompressionMethod, PhotometricInterpretation, Predictor, SampleFormat, Tag, Type,
};

use self::stream::{ByteOrder, EndianReader, SmartReader};

pub mod ifd;
mod image;
mod stream;
mod tag_reader;

/// Result of a decoding process
#[derive(Debug)]
pub enum DecodingResult {
    /// A vector of unsigned bytes
    U8(Vec<u8>),
    /// A vector of unsigned words
    U16(Vec<u16>),
    /// A vector of 32 bit unsigned ints
    U32(Vec<u32>),
    /// A vector of 64 bit unsigned ints
    U64(Vec<u64>),
    /// A vector of 32 bit IEEE floats
    F32(Vec<f32>),
    /// A vector of 64 bit IEEE floats
    F64(Vec<f64>),
    /// A vector of 8 bit signed ints
    I8(Vec<i8>),
    /// A vector of 16 bit signed ints
    I16(Vec<i16>),
    /// A vector of 32 bit signed ints
    I32(Vec<i32>),
    /// A vector of 64 bit signed ints
    I64(Vec<i64>),
}

impl DecodingResult {
    fn new_u8(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::U8(vec![0; size]))
        }
    }

    fn new_u16(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / 2 {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::U16(vec![0; size]))
        }
    }

    fn new_u32(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / 4 {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::U32(vec![0; size]))
        }
    }

    fn new_u64(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / 8 {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::U64(vec![0; size]))
        }
    }

    fn new_f32(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / std::mem::size_of::<f32>() {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::F32(vec![0.0; size]))
        }
    }

    fn new_f64(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / std::mem::size_of::<f64>() {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::F64(vec![0.0; size]))
        }
    }

    fn new_i8(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / std::mem::size_of::<i8>() {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::I8(vec![0; size]))
        }
    }

    fn new_i16(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / 2 {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::I16(vec![0; size]))
        }
    }

    fn new_i32(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / 4 {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::I32(vec![0; size]))
        }
    }

    fn new_i64(size: usize, limits: &Limits) -> TiffResult<DecodingResult> {
        if size > limits.decoding_buffer_size / 8 {
            Err(TiffError::LimitsExceeded)
        } else {
            Ok(DecodingResult::I64(vec![0; size]))
        }
    }

    pub fn as_buffer(&mut self, start: usize) -> DecodingBuffer {
        match *self {
            DecodingResult::U8(ref mut buf) => DecodingBuffer::U8(&mut buf[start..]),
            DecodingResult::U16(ref mut buf) => DecodingBuffer::U16(&mut buf[start..]),
            DecodingResult::U32(ref mut buf) => DecodingBuffer::U32(&mut buf[start..]),
            DecodingResult::U64(ref mut buf) => DecodingBuffer::U64(&mut buf[start..]),
            DecodingResult::F32(ref mut buf) => DecodingBuffer::F32(&mut buf[start..]),
            DecodingResult::F64(ref mut buf) => DecodingBuffer::F64(&mut buf[start..]),
            DecodingResult::I8(ref mut buf) => DecodingBuffer::I8(&mut buf[start..]),
            DecodingResult::I16(ref mut buf) => DecodingBuffer::I16(&mut buf[start..]),
            DecodingResult::I32(ref mut buf) => DecodingBuffer::I32(&mut buf[start..]),
            DecodingResult::I64(ref mut buf) => DecodingBuffer::I64(&mut buf[start..]),
        }
    }
}

// A buffer for image decoding
pub enum DecodingBuffer<'a> {
    /// A slice of unsigned bytes
    U8(&'a mut [u8]),
    /// A slice of unsigned words
    U16(&'a mut [u16]),
    /// A slice of 32 bit unsigned ints
    U32(&'a mut [u32]),
    /// A slice of 64 bit unsigned ints
    U64(&'a mut [u64]),
    /// A slice of 32 bit IEEE floats
    F32(&'a mut [f32]),
    /// A slice of 64 bit IEEE floats
    F64(&'a mut [f64]),
    /// A slice of 8 bits signed ints
    I8(&'a mut [i8]),
    /// A slice of 16 bits signed ints
    I16(&'a mut [i16]),
    /// A slice of 32 bits signed ints
    I32(&'a mut [i32]),
    /// A slice of 64 bits signed ints
    I64(&'a mut [i64]),
}

impl<'a> DecodingBuffer<'a> {
    fn byte_len(&self) -> usize {
        match *self {
            DecodingBuffer::U8(_) => 1,
            DecodingBuffer::U16(_) => 2,
            DecodingBuffer::U32(_) => 4,
            DecodingBuffer::U64(_) => 8,
            DecodingBuffer::F32(_) => 4,
            DecodingBuffer::F64(_) => 8,
            DecodingBuffer::I8(_) => 1,
            DecodingBuffer::I16(_) => 2,
            DecodingBuffer::I32(_) => 4,
            DecodingBuffer::I64(_) => 8,
        }
    }

    fn copy<'b>(&'b mut self) -> DecodingBuffer<'b>
    where
        'a: 'b,
    {
        match *self {
            DecodingBuffer::U8(ref mut buf) => DecodingBuffer::U8(buf),
            DecodingBuffer::U16(ref mut buf) => DecodingBuffer::U16(buf),
            DecodingBuffer::U32(ref mut buf) => DecodingBuffer::U32(buf),
            DecodingBuffer::U64(ref mut buf) => DecodingBuffer::U64(buf),
            DecodingBuffer::F32(ref mut buf) => DecodingBuffer::F32(buf),
            DecodingBuffer::F64(ref mut buf) => DecodingBuffer::F64(buf),
            DecodingBuffer::I8(ref mut buf) => DecodingBuffer::I8(buf),
            DecodingBuffer::I16(ref mut buf) => DecodingBuffer::I16(buf),
            DecodingBuffer::I32(ref mut buf) => DecodingBuffer::I32(buf),
            DecodingBuffer::I64(ref mut buf) => DecodingBuffer::I64(buf),
        }
    }

    fn subrange<'b>(&'b mut self, range: Range<usize>) -> DecodingBuffer<'b>
    where
        'a: 'b,
    {
        match *self {
            DecodingBuffer::U8(ref mut buf) => DecodingBuffer::U8(&mut buf[range]),
            DecodingBuffer::U16(ref mut buf) => DecodingBuffer::U16(&mut buf[range]),
            DecodingBuffer::U32(ref mut buf) => DecodingBuffer::U32(&mut buf[range]),
            DecodingBuffer::U64(ref mut buf) => DecodingBuffer::U64(&mut buf[range]),
            DecodingBuffer::F32(ref mut buf) => DecodingBuffer::F32(&mut buf[range]),
            DecodingBuffer::F64(ref mut buf) => DecodingBuffer::F64(&mut buf[range]),
            DecodingBuffer::I8(ref mut buf) => DecodingBuffer::I8(&mut buf[range]),
            DecodingBuffer::I16(ref mut buf) => DecodingBuffer::I16(&mut buf[range]),
            DecodingBuffer::I32(ref mut buf) => DecodingBuffer::I32(&mut buf[range]),
            DecodingBuffer::I64(ref mut buf) => DecodingBuffer::I64(&mut buf[range]),
        }
    }

    fn as_bytes_mut(&mut self) -> &mut [u8] {
        match self {
            DecodingBuffer::U8(buf) => &mut *buf,
            DecodingBuffer::I8(buf) => bytecast::i8_as_ne_mut_bytes(buf),
            DecodingBuffer::U16(buf) => bytecast::u16_as_ne_mut_bytes(buf),
            DecodingBuffer::I16(buf) => bytecast::i16_as_ne_mut_bytes(buf),
            DecodingBuffer::U32(buf) => bytecast::u32_as_ne_mut_bytes(buf),
            DecodingBuffer::I32(buf) => bytecast::i32_as_ne_mut_bytes(buf),
            DecodingBuffer::U64(buf) => bytecast::u64_as_ne_mut_bytes(buf),
            DecodingBuffer::I64(buf) => bytecast::i64_as_ne_mut_bytes(buf),
            DecodingBuffer::F32(buf) => bytecast::f32_as_ne_mut_bytes(buf),
            DecodingBuffer::F64(buf) => bytecast::f64_as_ne_mut_bytes(buf),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
/// Chunk type of the internal representation
pub enum ChunkType {
    Strip,
    Tile,
}

/// Decoding limits
#[derive(Clone, Debug)]
pub struct Limits {
    /// The maximum size of any `DecodingResult` in bytes, the default is
    /// 256MiB. If the entire image is decoded at once, then this will
    /// be the maximum size of the image. If it is decoded one strip at a
    /// time, this will be the maximum size of a strip.
    pub decoding_buffer_size: usize,
    /// The maximum size of any ifd value in bytes, the default is
    /// 1MiB.
    pub ifd_value_size: usize,
    /// Maximum size for intermediate buffer which may be used to limit the amount of data read per
    /// segment even if the entire image is decoded at once.
    pub intermediate_buffer_size: usize,
    /// The purpose of this is to prevent all the fields of the struct from
    /// being public, as this would make adding new fields a major version
    /// bump.
    _non_exhaustive: (),
}

impl Limits {
    /// A configuration that does not impose any limits.
    ///
    /// This is a good start if the caller only wants to impose selective limits, contrary to the
    /// default limits which allows selectively disabling limits.
    ///
    /// Note that this configuration is likely to crash on excessively large images since,
    /// naturally, the machine running the program does not have infinite memory.
    pub fn unlimited() -> Limits {
        Limits {
            decoding_buffer_size: usize::max_value(),
            ifd_value_size: usize::max_value(),
            intermediate_buffer_size: usize::max_value(),
            _non_exhaustive: (),
        }
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            decoding_buffer_size: 256 * 1024 * 1024,
            intermediate_buffer_size: 128 * 1024 * 1024,
            ifd_value_size: 1024 * 1024,
            _non_exhaustive: (),
        }
    }
}

/// The representation of a TIFF decoder
///
/// Currently does not support decoding of interlaced images
#[derive(Debug)]
pub struct Decoder<R>
where
    R: Read + Seek,
{
    reader: SmartReader<R>,
    bigtiff: bool,
    limits: Limits,
    next_ifd: Option<u64>,
    current_chunk: usize,
    seen_ifds: HashSet<u64>,
    image: Image,
}

trait Wrapping {
    fn wrapping_add(&self, other: Self) -> Self;
}

impl Wrapping for u8 {
    fn wrapping_add(&self, other: Self) -> Self {
        u8::wrapping_add(*self, other)
    }
}

impl Wrapping for u16 {
    fn wrapping_add(&self, other: Self) -> Self {
        u16::wrapping_add(*self, other)
    }
}

impl Wrapping for u32 {
    fn wrapping_add(&self, other: Self) -> Self {
        u32::wrapping_add(*self, other)
    }
}

impl Wrapping for u64 {
    fn wrapping_add(&self, other: Self) -> Self {
        u64::wrapping_add(*self, other)
    }
}

impl Wrapping for i8 {
    fn wrapping_add(&self, other: Self) -> Self {
        i8::wrapping_add(*self, other)
    }
}

impl Wrapping for i16 {
    fn wrapping_add(&self, other: Self) -> Self {
        i16::wrapping_add(*self, other)
    }
}

impl Wrapping for i32 {
    fn wrapping_add(&self, other: Self) -> Self {
        i32::wrapping_add(*self, other)
    }
}

impl Wrapping for i64 {
    fn wrapping_add(&self, other: Self) -> Self {
        i64::wrapping_add(*self, other)
    }
}

fn rev_hpredict_nsamp<T>(
    image: &mut [T],
    size: (u32, u32), // Size of the block
    img_width: usize, // Width of the image (this distinction is needed for tiles)
    samples: usize,
) -> TiffResult<()>
where
    T: Copy + Wrapping,
{
    let width = usize::try_from(size.0)?;
    let height = usize::try_from(size.1)?;
    for row in 0..height {
        for col in samples..width * samples {
            let prev_pixel = image[(row * img_width * samples + col - samples)];
            let pixel = &mut image[(row * img_width * samples + col)];
            *pixel = pixel.wrapping_add(prev_pixel);
        }
    }
    Ok(())
}

fn rev_hpredict(
    image: DecodingBuffer,
    size: (u32, u32),
    img_width: usize,
    color_type: ColorType,
) -> TiffResult<()> {
    // TODO: use bits_per_sample.len() after implementing type 3 predictor
    let samples = match color_type {
        ColorType::Gray(8) | ColorType::Gray(16) | ColorType::Gray(32) | ColorType::Gray(64) => 1,
        ColorType::RGB(8) | ColorType::RGB(16) | ColorType::RGB(32) | ColorType::RGB(64) => 3,
        ColorType::RGBA(8)
        | ColorType::RGBA(16)
        | ColorType::RGBA(32)
        | ColorType::RGBA(64)
        | ColorType::CMYK(8)
        | ColorType::CMYK(16)
        | ColorType::CMYK(32)
        | ColorType::CMYK(64) => 4,
        _ => {
            return Err(TiffError::UnsupportedError(
                TiffUnsupportedError::HorizontalPredictor(color_type),
            ))
        }
    };

    match image {
        DecodingBuffer::U8(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::U16(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::U32(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::U64(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::F32(_buf) => {
            // FIXME: check how this is defined.
            // See issue #89.
            // rev_hpredict_nsamp(buf, size, img_width,samples)?;
            return Err(TiffError::UnsupportedError(
                TiffUnsupportedError::HorizontalPredictor(color_type),
            ));
        }
        DecodingBuffer::F64(_buf) => {
            //FIXME: check how this is defined.
            // See issue #89.
            // rev_hpredict_nsamp(buf, size, img_width,samples)?;
            return Err(TiffError::UnsupportedError(
                TiffUnsupportedError::HorizontalPredictor(color_type),
            ));
        }
        DecodingBuffer::I8(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::I16(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::I32(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
        DecodingBuffer::I64(buf) => {
            rev_hpredict_nsamp(buf, size, img_width, samples)?;
        }
    }
    Ok(())
}

fn invert_colors_unsigned<T>(buffer: &mut [T], max: T)
where
    T: std::ops::Sub<T> + std::ops::Sub<Output = T> + Copy,
{
    for datum in buffer.iter_mut() {
        *datum = max - *datum
    }
}

fn invert_colors_fp<T>(buffer: &mut [T], max: T)
where
    T: std::ops::Sub<T> + std::ops::Sub<Output = T> + Copy,
{
    for datum in buffer.iter_mut() {
        // FIXME: assumes [0, 1) range for floats
        *datum = max - *datum
    }
}

fn invert_colors(buf: &mut DecodingBuffer, color_type: ColorType) {
    match (color_type, buf) {
        (ColorType::Gray(64), DecodingBuffer::U64(ref mut buffer)) => {
            invert_colors_unsigned(buffer, 0xffff_ffff_ffff_ffff);
        }
        (ColorType::Gray(32), DecodingBuffer::U32(ref mut buffer)) => {
            invert_colors_unsigned(buffer, 0xffff_ffff);
        }
        (ColorType::Gray(16), DecodingBuffer::U16(ref mut buffer)) => {
            invert_colors_unsigned(buffer, 0xffff);
        }
        (ColorType::Gray(n), DecodingBuffer::U8(ref mut buffer)) if n <= 8 => {
            invert_colors_unsigned(buffer, 0xff);
        }
        (ColorType::Gray(32), DecodingBuffer::F32(ref mut buffer)) => {
            invert_colors_fp(buffer, 1.0);
        }
        (ColorType::Gray(64), DecodingBuffer::F64(ref mut buffer)) => {
            invert_colors_fp(buffer, 1.0);
        }
        _ => {}
    }
}

/// Fix endianness. If `byte_order` matches the host, then conversion is a no-op.
fn fix_endianness(buf: &mut DecodingBuffer, byte_order: ByteOrder) {
    match byte_order {
        ByteOrder::LittleEndian => match buf {
            DecodingBuffer::U8(_) | DecodingBuffer::I8(_) => {}
            DecodingBuffer::U16(b) => b.iter_mut().for_each(|v| *v = u16::from_le(*v)),
            DecodingBuffer::I16(b) => b.iter_mut().for_each(|v| *v = i16::from_le(*v)),
            DecodingBuffer::U32(b) => b.iter_mut().for_each(|v| *v = u32::from_le(*v)),
            DecodingBuffer::I32(b) => b.iter_mut().for_each(|v| *v = i32::from_le(*v)),
            DecodingBuffer::U64(b) => b.iter_mut().for_each(|v| *v = u64::from_le(*v)),
            DecodingBuffer::I64(b) => b.iter_mut().for_each(|v| *v = i64::from_le(*v)),
            DecodingBuffer::F32(b) => b
                .iter_mut()
                .for_each(|v| *v = f32::from_bits(u32::from_le(v.to_bits()))),
            DecodingBuffer::F64(b) => b
                .iter_mut()
                .for_each(|v| *v = f64::from_bits(u64::from_le(v.to_bits()))),
        },
        ByteOrder::BigEndian => match buf {
            DecodingBuffer::U8(_) | DecodingBuffer::I8(_) => {}
            DecodingBuffer::U16(b) => b.iter_mut().for_each(|v| *v = u16::from_be(*v)),
            DecodingBuffer::I16(b) => b.iter_mut().for_each(|v| *v = i16::from_be(*v)),
            DecodingBuffer::U32(b) => b.iter_mut().for_each(|v| *v = u32::from_be(*v)),
            DecodingBuffer::I32(b) => b.iter_mut().for_each(|v| *v = i32::from_be(*v)),
            DecodingBuffer::U64(b) => b.iter_mut().for_each(|v| *v = u64::from_be(*v)),
            DecodingBuffer::I64(b) => b.iter_mut().for_each(|v| *v = i64::from_be(*v)),
            DecodingBuffer::F32(b) => b
                .iter_mut()
                .for_each(|v| *v = f32::from_bits(u32::from_be(v.to_bits()))),
            DecodingBuffer::F64(b) => b
                .iter_mut()
                .for_each(|v| *v = f64::from_bits(u64::from_be(v.to_bits()))),
        },
    };
}

impl<R: Read + Seek> Decoder<R> {
    /// Create a new decoder that decodes from the stream ```r```
    pub fn new(mut r: R) -> TiffResult<Decoder<R>> {
        let mut endianess = Vec::with_capacity(2);
        (&mut r).take(2).read_to_end(&mut endianess)?;
        let byte_order = match &*endianess {
            b"II" => ByteOrder::LittleEndian,
            b"MM" => ByteOrder::BigEndian,
            _ => {
                return Err(TiffError::FormatError(
                    TiffFormatError::TiffSignatureNotFound,
                ))
            }
        };
        let mut reader = SmartReader::wrap(r, byte_order);

        let bigtiff = match reader.read_u16()? {
            42 => false,
            43 => {
                // Read bytesize of offsets (in bigtiff it's alway 8 but provide a way to move to 16 some day)
                if reader.read_u16()? != 8 {
                    return Err(TiffError::FormatError(
                        TiffFormatError::TiffSignatureNotFound,
                    ));
                }
                // This constant should always be 0
                if reader.read_u16()? != 0 {
                    return Err(TiffError::FormatError(
                        TiffFormatError::TiffSignatureNotFound,
                    ));
                }
                true
            }
            _ => {
                return Err(TiffError::FormatError(
                    TiffFormatError::TiffSignatureInvalid,
                ))
            }
        };
        let next_ifd = if bigtiff {
            Some(reader.read_u64()?)
        } else {
            Some(u64::from(reader.read_u32()?))
        };

        let mut seen_ifds = HashSet::new();
        seen_ifds.insert(*next_ifd.as_ref().unwrap());

        let mut decoder = Decoder {
            reader,
            bigtiff,
            limits: Default::default(),
            next_ifd,
            image: Image {
                ifd: None,
                width: 0,
                height: 0,
                bits_per_sample: vec![1],
                samples: 1,
                sample_format: vec![SampleFormat::Uint],
                photometric_interpretation: PhotometricInterpretation::BlackIsZero,
                compression_method: CompressionMethod::None,
                jpeg_tables: None,
                predictor: Predictor::None,
                chunk_type: ChunkType::Strip,
                strip_decoder: None,
                tile_attributes: None,
                chunk_offsets: Vec::new(),
                chunk_bytes: Vec::new(),
            },
            current_chunk: 0,
            seen_ifds,
        };
        decoder.next_image()?;
        Ok(decoder)
    }

    pub fn with_limits(mut self, limits: Limits) -> Decoder<R> {
        self.limits = limits;
        self
    }

    pub fn dimensions(&mut self) -> TiffResult<(u32, u32)> {
        Ok((self.image().width, self.image().height))
    }

    pub fn colortype(&mut self) -> TiffResult<ColorType> {
        self.image().colortype()
    }

    fn image(&self) -> &Image {
        &self.image
    }

    /// Reset the decoder.
    #[deprecated = "Never should have been public. Only use Decoder::new()"]
    pub fn init(self) -> TiffResult<Decoder<R>> {
        let Self { reader, .. } = self;
        Self::new(reader.into_inner())
    }

    /// Reads in the next image.
    /// If there is no further image in the TIFF file a format error is returned.
    /// To determine whether there are more images call `TIFFDecoder::more_images` instead.
    pub fn next_image(&mut self) -> TiffResult<()> {
        if self.next_ifd.is_none() {
            return Err(TiffError::FormatError(
                TiffFormatError::ImageFileDirectoryNotFound,
            ));
        }

        let (ifd, next_ifd) = Self::read_ifd(
            &mut self.reader,
            self.bigtiff,
            self.next_ifd.take().unwrap(),
        )?;

        if let Some(next) = next_ifd {
            if !self.seen_ifds.insert(next) {
                return Err(TiffError::FormatError(TiffFormatError::CycleInOffsets));
            }
            self.next_ifd = Some(next);
        }

        self.current_chunk = 0;
        self.image = Image::from_reader(&mut self.reader, ifd, &self.limits, self.bigtiff)?;
        Ok(())
    }

    /// Returns `true` if there is at least one more image available.
    pub fn more_images(&self) -> bool {
        self.next_ifd.is_some()
    }

    /// Returns the byte_order
    pub fn byte_order(&self) -> ByteOrder {
        self.reader.byte_order
    }

    #[inline]
    pub fn read_ifd_offset(&mut self) -> Result<u64, io::Error> {
        if self.bigtiff {
            self.read_long8()
        } else {
            self.read_long().map(u64::from)
        }
    }

    /// Reads a TIFF byte value
    #[inline]
    pub fn read_byte(&mut self) -> Result<u8, io::Error> {
        let mut buf = [0; 1];
        self.reader.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    /// Reads a TIFF short value
    #[inline]
    pub fn read_short(&mut self) -> Result<u16, io::Error> {
        self.reader.read_u16()
    }

    /// Reads a TIFF sshort value
    #[inline]
    pub fn read_sshort(&mut self) -> Result<i16, io::Error> {
        self.reader.read_i16()
    }

    /// Reads a TIFF long value
    #[inline]
    pub fn read_long(&mut self) -> Result<u32, io::Error> {
        self.reader.read_u32()
    }

    /// Reads a TIFF slong value
    #[inline]
    pub fn read_slong(&mut self) -> Result<i32, io::Error> {
        self.reader.read_i32()
    }

    /// Reads a TIFF float value
    #[inline]
    pub fn read_float(&mut self) -> Result<f32, io::Error> {
        self.reader.read_f32()
    }

    /// Reads a TIFF double value
    #[inline]
    pub fn read_double(&mut self) -> Result<f64, io::Error> {
        self.reader.read_f64()
    }

    #[inline]
    pub fn read_long8(&mut self) -> Result<u64, io::Error> {
        self.reader.read_u64()
    }

    #[inline]
    pub fn read_slong8(&mut self) -> Result<i64, io::Error> {
        self.reader.read_i64()
    }

    /// Reads a string
    #[inline]
    pub fn read_string(&mut self, length: usize) -> TiffResult<String> {
        let mut out = vec![0; length];
        self.reader.read_exact(&mut out)?;
        // Strings may be null-terminated, so we trim anything downstream of the null byte
        if let Some(first) = out.iter().position(|&b| b == 0) {
            out.truncate(first);
        }
        Ok(String::from_utf8(out)?)
    }

    /// Reads a TIFF IFA offset/value field
    #[inline]
    pub fn read_offset(&mut self) -> TiffResult<[u8; 4]> {
        if self.bigtiff {
            return Err(TiffError::FormatError(
                TiffFormatError::InconsistentSizesEncountered,
            ));
        }
        let mut val = [0; 4];
        self.reader.read_exact(&mut val)?;
        Ok(val)
    }

    /// Reads a TIFF IFA offset/value field
    #[inline]
    pub fn read_offset_u64(&mut self) -> Result<[u8; 8], io::Error> {
        let mut val = [0; 8];
        self.reader.read_exact(&mut val)?;
        Ok(val)
    }

    /// Moves the cursor to the specified offset
    #[inline]
    pub fn goto_offset(&mut self, offset: u32) -> io::Result<()> {
        self.goto_offset_u64(offset.into())
    }

    #[inline]
    pub fn goto_offset_u64(&mut self, offset: u64) -> io::Result<()> {
        self.reader.seek(io::SeekFrom::Start(offset)).map(|_| ())
    }

    /// Reads a IFD entry.
    // An IFD entry has four fields:
    //
    // Tag   2 bytes
    // Type  2 bytes
    // Count 4 bytes
    // Value 4 bytes either a pointer the value itself
    fn read_entry(
        reader: &mut SmartReader<R>,
        bigtiff: bool,
    ) -> TiffResult<Option<(Tag, ifd::Entry)>> {
        let tag = Tag::from_u16_exhaustive(reader.read_u16()?);
        let type_ = match Type::from_u16(reader.read_u16()?) {
            Some(t) => t,
            None => {
                // Unknown type. Skip this entry according to spec.
                reader.read_u32()?;
                reader.read_u32()?;
                return Ok(None);
            }
        };
        let entry = if bigtiff {
            let mut offset = [0; 8];

            let count = reader.read_u64()?;
            reader.read_exact(&mut offset)?;
            ifd::Entry::new_u64(type_, count, offset)
        } else {
            let mut offset = [0; 4];

            let count = reader.read_u32()?;
            reader.read_exact(&mut offset)?;
            ifd::Entry::new(type_, count, offset)
        };
        Ok(Some((tag, entry)))
    }

    /// Reads the IFD starting at the indicated location.
    fn read_ifd(
        reader: &mut SmartReader<R>,
        bigtiff: bool,
        ifd_location: u64,
    ) -> TiffResult<(Directory, Option<u64>)> {
        reader.goto_offset(ifd_location)?;

        let mut dir: Directory = HashMap::new();

        let num_tags = if bigtiff {
            reader.read_u64()?
        } else {
            reader.read_u16()?.into()
        };
        for _ in 0..num_tags {
            let (tag, entry) = match Self::read_entry(reader, bigtiff)? {
                Some(val) => val,
                None => {
                    continue;
                } // Unknown data type in tag, skip
            };
            dir.insert(tag, entry);
        }

        let next_ifd = if bigtiff {
            reader.read_u64()?
        } else {
            reader.read_u32()?.into()
        };

        let next_ifd = match next_ifd {
            0 => None,
            _ => Some(next_ifd),
        };

        Ok((dir, next_ifd))
    }

    /// Tries to retrieve a tag.
    /// Return `Ok(None)` if the tag is not present.
    pub fn find_tag(&mut self, tag: Tag) -> TiffResult<Option<ifd::Value>> {
        let entry = match self.image().ifd.as_ref().unwrap().get(&tag) {
            None => return Ok(None),
            Some(entry) => entry.clone(),
        };

        Ok(Some(entry.val(
            &self.limits,
            self.bigtiff,
            &mut self.reader,
        )?))
    }

    /// Tries to retrieve a tag and convert it to the desired unsigned type.
    pub fn find_tag_unsigned<T: TryFrom<u64>>(&mut self, tag: Tag) -> TiffResult<Option<T>> {
        self.find_tag(tag)?
            .map(|v| v.into_u64())
            .transpose()?
            .map(|value| {
                T::try_from(value).map_err(|_| TiffFormatError::InvalidTagValueType(tag).into())
            })
            .transpose()
    }

    /// Tries to retrieve a vector of all a tag's values and convert them to
    /// the desired unsigned type.
    pub fn find_tag_unsigned_vec<T: TryFrom<u64>>(
        &mut self,
        tag: Tag,
    ) -> TiffResult<Option<Vec<T>>> {
        self.find_tag(tag)?
            .map(|v| v.into_u64_vec())
            .transpose()?
            .map(|v| {
                v.into_iter()
                    .map(|u| {
                        T::try_from(u).map_err(|_| TiffFormatError::InvalidTagValueType(tag).into())
                    })
                    .collect()
            })
            .transpose()
    }

    /// Tries to retrieve a tag and convert it to the desired unsigned type.
    /// Returns an error if the tag is not present.
    pub fn get_tag_unsigned<T: TryFrom<u64>>(&mut self, tag: Tag) -> TiffResult<T> {
        self.find_tag_unsigned(tag)?
            .ok_or_else(|| TiffFormatError::RequiredTagNotFound(tag).into())
    }

    /// Tries to retrieve a tag.
    /// Returns an error if the tag is not present
    pub fn get_tag(&mut self, tag: Tag) -> TiffResult<ifd::Value> {
        match self.find_tag(tag)? {
            Some(val) => Ok(val),
            None => Err(TiffError::FormatError(
                TiffFormatError::RequiredTagNotFound(tag),
            )),
        }
    }

    /// Tries to retrieve a tag and convert it to the desired type.
    pub fn get_tag_u32(&mut self, tag: Tag) -> TiffResult<u32> {
        self.get_tag(tag)?.into_u32()
    }
    pub fn get_tag_u64(&mut self, tag: Tag) -> TiffResult<u64> {
        self.get_tag(tag)?.into_u64()
    }

    /// Tries to retrieve a tag and convert it to the desired type.
    pub fn get_tag_f32(&mut self, tag: Tag) -> TiffResult<f32> {
        self.get_tag(tag)?.into_f32()
    }

    /// Tries to retrieve a tag and convert it to the desired type.
    pub fn get_tag_f64(&mut self, tag: Tag) -> TiffResult<f64> {
        self.get_tag(tag)?.into_f64()
    }

    /// Tries to retrieve a tag and convert it to the desired type.
    pub fn get_tag_u32_vec(&mut self, tag: Tag) -> TiffResult<Vec<u32>> {
        self.get_tag(tag)?.into_u32_vec()
    }

    pub fn get_tag_u16_vec(&mut self, tag: Tag) -> TiffResult<Vec<u16>> {
        self.get_tag(tag)?.into_u16_vec()
    }
    pub fn get_tag_u64_vec(&mut self, tag: Tag) -> TiffResult<Vec<u64>> {
        self.get_tag(tag)?.into_u64_vec()
    }

    /// Tries to retrieve a tag and convert it to the desired type.
    pub fn get_tag_f32_vec(&mut self, tag: Tag) -> TiffResult<Vec<f32>> {
        self.get_tag(tag)?.into_f32_vec()
    }

    /// Tries to retrieve a tag and convert it to the desired type.
    pub fn get_tag_f64_vec(&mut self, tag: Tag) -> TiffResult<Vec<f64>> {
        self.get_tag(tag)?.into_f64_vec()
    }

    /// Tries to retrieve a tag and convert it to a 8bit vector.
    pub fn get_tag_u8_vec(&mut self, tag: Tag) -> TiffResult<Vec<u8>> {
        self.get_tag(tag)?.into_u8_vec()
    }

    /// Tries to retrieve a tag and convert it to a ascii vector.
    pub fn get_tag_ascii_string(&mut self, tag: Tag) -> TiffResult<String> {
        self.get_tag(tag)?.into_string()
    }

    fn check_chunk_type(&self, expected: ChunkType) -> TiffResult<()> {
        if expected != self.image().chunk_type {
            return Err(TiffError::UsageError(UsageError::InvalidChunkType(
                expected,
                self.image().chunk_type,
            )));
        }

        Ok(())
    }

    /// The chunk type (Strips / Tiles) of the image
    pub fn get_chunk_type(&self) -> ChunkType {
        self.image().chunk_type
    }

    /// Number of strips in image
    pub fn strip_count(&mut self) -> TiffResult<u32> {
        self.check_chunk_type(ChunkType::Strip)?;
        let rows_per_strip = self.image().strip_decoder.as_ref().unwrap().rows_per_strip;

        if rows_per_strip == 0 {
            return Ok(0);
        }

        // rows_per_strip - 1 can never fail since we know it's at least 1
        let height = match self.image().height.checked_add(rows_per_strip - 1) {
            Some(h) => h,
            None => return Err(TiffError::IntSizeError),
        };

        Ok(height / rows_per_strip)
    }

    /// Number of tiles in image
    pub fn tile_count(&mut self) -> TiffResult<u32> {
        self.check_chunk_type(ChunkType::Tile)?;
        Ok(u32::try_from(self.image().chunk_offsets.len())?)
    }

    #[deprecated = "Use read_image instead"]
    pub fn read_jpeg(&mut self) -> TiffResult<DecodingResult> {
        self.read_image()
    }

    pub fn read_strip_to_buffer(&mut self, mut buffer: DecodingBuffer) -> TiffResult<()> {
        self.check_chunk_type(ChunkType::Strip)?;

        let offset = self.image.chunk_file_range(self.current_chunk)?.0;
        self.goto_offset_u64(offset)?;

        let byte_order = self.reader.byte_order;
        let output_width = usize::try_from(self.image().width)?;
        self.image.expand_chunk(
            &mut self.reader,
            buffer.copy(),
            output_width,
            byte_order,
            self.current_chunk,
        )?;

        self.current_chunk += 1;

        Ok(())
    }

    fn result_buffer(&self, width: usize, height: usize) -> TiffResult<DecodingResult> {
        let buffer_size = match width
            .checked_mul(height)
            .and_then(|x| x.checked_mul(self.image().bits_per_sample.len()))
        {
            Some(s) => s,
            None => return Err(TiffError::LimitsExceeded),
        };

        let max_sample_bits = self
            .image()
            .bits_per_sample
            .iter()
            .cloned()
            .max()
            .unwrap_or(8);
        match self
            .image()
            .sample_format
            .first()
            .unwrap_or(&SampleFormat::Uint)
        {
            SampleFormat::Uint => match max_sample_bits {
                n if n <= 8 => DecodingResult::new_u8(buffer_size, &self.limits),
                n if n <= 16 => DecodingResult::new_u16(buffer_size, &self.limits),
                n if n <= 32 => DecodingResult::new_u32(buffer_size, &self.limits),
                n if n <= 64 => DecodingResult::new_u64(buffer_size, &self.limits),
                n => Err(TiffError::UnsupportedError(
                    TiffUnsupportedError::UnsupportedBitsPerChannel(n),
                )),
            },
            SampleFormat::IEEEFP => match max_sample_bits {
                32 => DecodingResult::new_f32(buffer_size, &self.limits),
                64 => DecodingResult::new_f64(buffer_size, &self.limits),
                n => Err(TiffError::UnsupportedError(
                    TiffUnsupportedError::UnsupportedBitsPerChannel(n),
                )),
            },
            SampleFormat::Int => match max_sample_bits {
                n if n <= 8 => DecodingResult::new_i8(buffer_size, &self.limits),
                n if n <= 16 => DecodingResult::new_i16(buffer_size, &self.limits),
                n if n <= 32 => DecodingResult::new_i32(buffer_size, &self.limits),
                n if n <= 64 => DecodingResult::new_i64(buffer_size, &self.limits),
                n => Err(TiffError::UnsupportedError(
                    TiffUnsupportedError::UnsupportedBitsPerChannel(n),
                )),
            },
            format => {
                Err(TiffUnsupportedError::UnsupportedSampleFormat(vec![format.clone()]).into())
            }
        }
    }

    /// Read a single strip from the image and return it as a Vector
    pub fn read_strip(&mut self) -> TiffResult<DecodingResult> {
        self.check_chunk_type(ChunkType::Strip)?;
        let index = self.current_chunk;

        let rows_per_strip =
            usize::try_from(self.image().strip_decoder.as_ref().unwrap().rows_per_strip)?;

        let strip_height = cmp::min(
            rows_per_strip,
            usize::try_from(self.image().height)? - index * rows_per_strip,
        );

        let mut result = self.result_buffer(usize::try_from(self.image().width)?, strip_height)?;
        self.read_strip_to_buffer(result.as_buffer(0))?;

        Ok(result)
    }

    /// Read a single tile from the image and return it as a Vector
    pub fn read_tile(&mut self) -> TiffResult<DecodingResult> {
        self.check_chunk_type(ChunkType::Tile)?;

        let tile = self.current_chunk;

        let tile_attrs = self.image().tile_attributes.as_ref().unwrap();
        let (padding_right, padding_down) = tile_attrs.get_padding(tile);

        let tile_width = tile_attrs.tile_width - padding_right;
        let tile_length = tile_attrs.tile_length - padding_down;

        let mut result = self.result_buffer(tile_width, tile_length)?;

        let offset = self.image.chunk_file_range(tile)?.0;
        self.goto_offset_u64(offset)?;

        let byte_order = self.reader.byte_order;
        self.image.expand_chunk(
            &mut self.reader,
            result.as_buffer(0),
            tile_width,
            byte_order,
            tile,
        )?;

        self.current_chunk += 1;

        Ok(result)
    }

    /// Decodes the entire image and return it as a Vector
    pub fn read_image(&mut self) -> TiffResult<DecodingResult> {
        let width = usize::try_from(self.image().width)?;
        let height = usize::try_from(self.image().height)?;
        let mut result = self.result_buffer(width, height)?;
        if width == 0 || height == 0 {
            return Ok(result);
        }

        let chunk_dimensions = self.image().chunk_dimensions()?;
        let chunk_dimensions = (
            chunk_dimensions.0.min(width),
            chunk_dimensions.1.min(height),
        );
        if chunk_dimensions.0 == 0 || chunk_dimensions.1 == 0 {
            return Err(TiffError::FormatError(
                TiffFormatError::InconsistentSizesEncountered,
            ));
        }

        let samples = self.image().bits_per_sample.len();
        if samples == 0 {
            return Err(TiffError::FormatError(
                TiffFormatError::InconsistentSizesEncountered,
            ));
        }

        let chunks_across = (width - 1) / chunk_dimensions.0 + 1;
        let strip_samples = width * chunk_dimensions.1 * samples;

        for chunk in 0..self.image().chunk_offsets.len() {
            self.goto_offset_u64(self.image().chunk_offsets[chunk])?;

            let x = chunk % chunks_across;
            let y = chunk / chunks_across;
            let buffer_offset = y * strip_samples + x * chunk_dimensions.0 * samples;
            let byte_order = self.reader.byte_order;
            self.image.expand_chunk(
                &mut self.reader,
                result.as_buffer(buffer_offset).copy(),
                width,
                byte_order,
                chunk,
            )?;
        }

        Ok(result)
    }
}
