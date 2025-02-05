use crate::series_ids::{LeftRight, MergedSeriesIds, SeriesIdArray, SeriesIdSlice};

use bitvec::prelude::*;
use zerocopy_derive::*;

use error::NdError;

/// Fixed-size buffer implementation of the Gorilla time series compression algorithm.
/// Provides efficient storage and compression of multiple time series data streams
/// in a memory-aligned format.
///
/// # Memory Layout
/// ```text
/// Fixed-Size Tabular Gorilla Buffer Layout (Example with 3 time series)
/// -------------------------------------------------------------------
///
/// Total Buffer Size = N bytes (must be multiple of 4)
///
/// +---------------------------------+ <-- 4-byte aligned
/// | Timestamp (4 bytes)             | Non-zero seconds since Unix epoch
/// +---------------------------------+ <-- 4-byte aligned
/// | Collection iterations (2 bytes) |
/// +---------------------------------+
/// | Number of series (2 bytes)      |
/// +---------------------------------+ <-- 4-byte aligned
/// | Series id 1 (4 bytes)           |
/// | Series id 2 (4 bytes)           | N number of series ids
/// | Series id 3 (4 bytes)           |
/// +---------------------------------+ <-- 4-byte aligned
/// | Initial value 1 (4 bytes)       |
/// | Initial value 2 (4 bytes)       | Initial value of each series
/// | Initial value 3 (4 bytes)       |
/// +---------------------------------+ <-- 4-byte aligned
/// |                                 |
/// | Available space for compressed  |
/// | Compressed Data                 |
/// |                                 |
/// |                                 |
/// |                                 |
/// |                                 |
/// |                                 |
/// +--------------------------------+ <-- 4-byte aligned
/// ```
///
#[derive(TryFromBytes, Debug)]
#[repr(C)]
pub struct GorillaBuffer<const NUM_SERIES: usize, const PAYLOAD_BYTES: usize> {
    /// Non-zero seconds since Unix epoch
    timestamp: std::num::NonZero<u32>,

    /// Number of collection iterations completed
    iterations: u16,

    /// Array of unique identifiers for each time series
    series_ids: SeriesIdArray<NUM_SERIES>,

    /// Starting values for each series
    initial_values: [u32; NUM_SERIES],

    /// Compressed data storage
    payload: [u8; PAYLOAD_BYTES],
}

impl<const N: usize, const P: usize> GorillaBuffer<N, P> {
    /// Create a new gorilla buffer that will store data from the given
    /// series ids. Remaining capacity `N - num series ids` will be left
    /// unused.
    pub fn new(
        timestamp: std::num::NonZeroU32,
        series_ids: &SeriesIdSlice,
        initial_values: &[u32],
    ) -> Result<Self, NdError> {
        if initial_values.len() > N {
            return Err(NdError::InvalidInitialValues(initial_values.len()));
        }
        let mut buffer_initial_values = [0; N];
        buffer_initial_values[..initial_values.len()].copy_from_slice(initial_values);

        Ok(Self {
            timestamp,
            iterations: 0,
            series_ids: SeriesIdArray::from_slice(series_ids),
            initial_values: buffer_initial_values,
            payload: [0; P],
        })
    }

    /// Number of series stored in this buffer
    pub fn num_series(&self) -> usize {
        self.series_ids.len()
    }

    /// A gorilla buffer is always created with at least one series id, hence
    /// this will always return false.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn initial_values(&self) -> &[u32] {
        &self.initial_values[..self.num_series()]
    }
}

#[derive(Debug)]
pub struct GorillaWriter<'a, const N: usize, const P: usize> {
    buffer: &'a mut GorillaBuffer<N, P>,
    write_offset_bits: usize,
    prev_lzc: [u32; N],
    prev_values: [u32; N],
}

impl<'a, const N: usize, const P: usize> GorillaWriter<'a, N, P> {
    pub fn new(buffer: &'a mut GorillaBuffer<N, P>) -> Self {
        let prev_values = buffer.initial_values;
        Self {
            buffer,
            write_offset_bits: 0,
            prev_lzc: [0; N],
            prev_values,
        }
    }

    pub fn add_samples(&mut self, curr_values: &[u32]) -> Result<(), NdError> {
        debug_assert_eq!(curr_values.len(), self.buffer.num_series());
        debug_assert!(self.buffer.iterations < u16::MAX);

        let payload = self.buffer.payload.view_bits_mut::<Msb0>();

        let iter = self.prev_values.iter().zip(curr_values.iter()).enumerate();
        for (idx, (&prev_value, &curr_value)) in iter {
            let prev_write_offset_bits = self.write_offset_bits;

            // The worst case scenario for gorilla is writting 39 bits for a u32
            // value: [same_numbers_bit (1b), same_lzc (1b), lzc_value (5), meaningful_value(32-5)b]
            if payload[self.write_offset_bits..].len() <= 39 {
                self.write_offset_bits = prev_write_offset_bits;
                return Err(NdError::NoSpace);
            }
            let xor = curr_value ^ prev_value;

            payload.set(self.write_offset_bits, xor != 0);
            self.write_offset_bits += 1;

            if xor != 0 {
                let lzc_prev = self.prev_lzc[idx];
                let lzc_curr = xor.leading_zeros();

                payload.set(self.write_offset_bits, lzc_prev != lzc_curr);
                self.write_offset_bits += 1;

                if lzc_prev != lzc_curr {
                    let r = self.write_offset_bits..self.write_offset_bits + 5;
                    payload[r].store_be(lzc_curr);
                    self.write_offset_bits += 5;

                    self.prev_lzc[idx] = lzc_curr;
                }

                let meaningful_bits = 32 - lzc_curr as usize;
                let r = self.write_offset_bits..self.write_offset_bits + meaningful_bits;
                payload[r.clone()].store_be(xor);

                self.write_offset_bits += meaningful_bits;
            }
        }

        self.prev_values[..curr_values.len()].copy_from_slice(curr_values);
        self.buffer.iterations += 1;
        Ok(())
    }

    pub fn add_series_ids(&mut self, series_ids: SeriesIdSlice) -> Result<(), NdError> {
        let num_new_series = series_ids.len();

        let mut gr = GorillaRelocator::new();
        gr.fixup(
            self.buffer,
            self.write_offset_bits,
            series_ids,
            &mut self.prev_lzc,
            &mut self.prev_values,
        )?;

        self.write_offset_bits += num_new_series * self.buffer.iterations as usize;
        Ok(())
    }

    pub fn compression_ratio(&self) -> f64 {
        let compressed_payload_size = (self.write_offset_bits + 7) / 8;
        let uncompressed_payload_size =
            self.buffer.iterations as usize * self.buffer.num_series() * std::mem::size_of::<u32>();

        compressed_payload_size as f64 / uncompressed_payload_size as f64
    }
}

pub struct GorillaReader<'a, const N: usize, const P: usize> {
    buffer: &'a GorillaBuffer<N, P>,
    num_series: usize,
    read_offset_bits: usize,
    curr_lzc: [u32; N],
    curr_values: [u32; N],
    iteration: u16,
}

impl<'a, const N: usize, const P: usize> GorillaReader<'a, N, P> {
    pub fn new(buffer: &'a GorillaBuffer<N, P>) -> Self {
        Self {
            buffer,
            num_series: buffer.num_series(),
            read_offset_bits: 0,
            curr_lzc: [0; N],
            curr_values: buffer.initial_values,
            iteration: 0,
        }
    }

    pub fn read_samples(&mut self) -> Option<&[u32]> {
        if self.iteration >= self.buffer.iterations {
            return None;
        }

        let payload = self.buffer.payload.view_bits::<Msb0>();

        for (idx, value) in (0..self.num_series).zip(self.curr_values.iter_mut()) {
            let value_changed = payload[self.read_offset_bits];
            self.read_offset_bits += 1;

            if value_changed {
                let lzc_changed = payload[self.read_offset_bits];
                self.read_offset_bits += 1;

                if lzc_changed {
                    let r = self.read_offset_bits..self.read_offset_bits + 5;
                    self.curr_lzc[idx] = payload[r].load_be();
                    self.read_offset_bits += 5;
                }

                let meaningful_bits = 32 - self.curr_lzc[idx] as usize;
                let r = self.read_offset_bits..self.read_offset_bits + meaningful_bits;
                let xor_value = payload[r].load_be::<u32>();
                self.read_offset_bits += meaningful_bits;

                *value ^= xor_value;
            }
        }

        self.iteration += 1;
        Some(&self.curr_values[..self.num_series])
    }
}

#[allow(dead_code)]
struct GorillaRelocator<const N: usize, const P: usize> {
    prev_lzc: [u32; N],
}

#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
struct RelocationItemInfo {
    num_bits: usize,
    lzc: u32,
}

#[derive(Debug)]
struct BitCursor {
    read_offset_bits: usize,
    write_offset_bits: usize,
}

#[derive(Default, Debug)]
struct SampleInfo {
    value_changed: bool,
    lzc_changed: bool,
    lzc: u32,
    meaningful_bits: usize,
    lzc_range: std::ops::Range<usize>,
    meaningful_bits_range: std::ops::Range<usize>,
}

impl<const N: usize, const P: usize> GorillaRelocator<N, P> {
    fn new() -> Self {
        Self { prev_lzc: [0; N] }
    }

    fn collect_relocation_information(
        payload_bits: &BitSlice<u8, Msb0>,
        bit_cursor: &BitCursor,
        rifs: &mut [RelocationItemInfo],
    ) {
        let mut p = bit_cursor.read_offset_bits;

        for rif in rifs.iter_mut() {
            let start_pos = p;

            let mut sample_info = SampleInfo::default();

            sample_info.value_changed = payload_bits[p];
            p += 1;

            if sample_info.value_changed {
                sample_info.lzc_changed = payload_bits[p];
                p += 1;

                if sample_info.lzc_changed {
                    sample_info.lzc_range = p..p + 5;
                    let b0 = payload_bits[p] as u32;
                    let b1 = payload_bits[p + 1] as u32;
                    let b2 = payload_bits[p + 2] as u32;
                    let b3 = payload_bits[p + 3] as u32;
                    let b4 = payload_bits[p + 4] as u32;

                    // Combine the bits, treating them as MSB->LSB
                    sample_info.lzc = (b0 << 4) | (b1 << 3) | (b2 << 2) | (b3 << 1) | b4;

                    p += 5;
                } else {
                    sample_info.lzc = rif.lzc;
                }

                sample_info.meaningful_bits = 32 - sample_info.lzc as usize;
                sample_info.meaningful_bits_range = p..p + sample_info.meaningful_bits;

                p += sample_info.meaningful_bits;
            }

            let end_pos = p;

            rif.num_bits = end_pos - start_pos;
            rif.lzc = sample_info.lzc;
        }
    }

    fn process_relocation(
        payload_bits: &mut BitSlice<u8, Msb0>,
        bit_cursor: &mut BitCursor,
        rifs: &[RelocationItemInfo],
        merged_ids: MergedSeriesIds<'_, '_>,
    ) {
        let mut rif_idx = 0;

        for merged_id in merged_ids {
            match merged_id {
                LeftRight::Left(_) => {
                    let num_bits = rifs[rif_idx].num_bits;
                    // Copy the bits
                    let src = bit_cursor.read_offset_bits..bit_cursor.read_offset_bits + num_bits;
                    let dest = bit_cursor.write_offset_bits;
                    payload_bits.copy_within(src, dest);

                    bit_cursor.read_offset_bits += num_bits;
                    bit_cursor.write_offset_bits += num_bits;

                    rif_idx += 1;
                }
                LeftRight::Right(_) => {
                    // For new series, write a single 0-bit indicating no value change
                    // FIXME: this should be the EMPTY_SLOT value
                    payload_bits.set(bit_cursor.write_offset_bits, false);
                    bit_cursor.write_offset_bits += 1;
                }
            }
        }
    }

    pub fn fixup(
        &mut self,
        buffer: &mut GorillaBuffer<N, P>,
        write_offset_bits: usize,
        new_ids: SeriesIdSlice,
        prev_lzc: &mut [u32; N],
        prev_values: &mut [u32; N],
    ) -> Result<(), NdError> {
        if new_ids.len() + buffer.num_series() > N {
            return Err(NdError::SeriesIdsArrayFull);
        }

        if buffer.iterations > 0 {
            // bail out if we don't have enough space
            // FIXME: we need to add 7 bits?
            let payload_bits = buffer.payload.view_bits_mut::<Msb0>();
            let new_bits_required = new_ids.len() * buffer.iterations as usize;

            if write_offset_bits + new_bits_required > payload_bits.len() {
                return Err(NdError::NoSpace);
            }

            let mut bit_cursor = BitCursor {
                read_offset_bits: payload_bits.len() - write_offset_bits,
                write_offset_bits: 0,
            };

            // move written bits to the end of the payload
            let src = 0..write_offset_bits;
            payload_bits.copy_within(src.clone(), bit_cursor.read_offset_bits);

            // get the existing ids of the buffer
            let existing_ids = buffer.series_ids.as_slice();
            debug_assert_ne!(existing_ids.len(), 0);

            // mutable slice for the relocation information we need
            let mut rifs_array = [RelocationItemInfo {
                num_bits: 0,
                lzc: 0,
            }; N];
            let rifs = &mut rifs_array[..existing_ids.len()];

            // create a merged series id iterator
            let merged_ids = MergedSeriesIds::new(&existing_ids, &new_ids);

            // perform as many iterations as we've performed during compression
            for _ in 0..buffer.iterations as usize {
                // update the relocation information items
                Self::collect_relocation_information(payload_bits, &bit_cursor, rifs);

                // perform the relocation
                Self::process_relocation(payload_bits, &mut bit_cursor, rifs, merged_ids.clone());
            }
        }

        // update the series id array
        let mut series_ids = [0; N];
        for (idx, merged_id) in
            MergedSeriesIds::new(&buffer.series_ids.as_slice(), &new_ids).enumerate()
        {
            match merged_id {
                LeftRight::Left(series_id) | LeftRight::Right(series_id) => {
                    series_ids[idx] = series_id.get();
                }
            }
        }

        let mut prev_initial_values_index = 0;
        let mut initial_values = [0; N];
        let mut new_prev_lzcs = [0; N];
        let mut new_prev_values = [0; N];
        for (idx, merged_id) in
            MergedSeriesIds::new(&buffer.series_ids.as_slice(), &new_ids).enumerate()
        {
            match merged_id {
                LeftRight::Left(series_id) => {
                    series_ids[idx] = series_id.get();
                    initial_values[idx] = buffer.initial_values[prev_initial_values_index];
                    new_prev_lzcs[idx] = prev_lzc[prev_initial_values_index];
                    new_prev_values[idx] = prev_values[prev_initial_values_index];

                    prev_initial_values_index += 1;
                }
                LeftRight::Right(series_id) => {
                    series_ids[idx] = series_id.get();
                    initial_values[idx] = 0;
                    new_prev_lzcs[idx] = 0;
                    new_prev_values[idx] = 0;
                }
            }
        }

        buffer.series_ids = unsafe {
            SeriesIdArray::new_unchecked(&series_ids[..buffer.num_series() + new_ids.len()])
        };
        buffer.initial_values = initial_values;
        prev_lzc.copy_from_slice(&new_prev_lzcs);
        prev_values.copy_from_slice(&new_prev_values);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use std::num::NonZeroU32;

    // Helper function to create a test buffer
    fn create_test_buffer<const N: usize, const P: usize>(
        series_ids: &[u32],
        initial_values: &[u32],
    ) -> GorillaBuffer<N, P> {
        let timestamp = unsafe { NonZeroU32::new_unchecked(1000) };
        let series_slice = SeriesIdSlice::new(series_ids).unwrap();
        GorillaBuffer::new(timestamp, &series_slice, initial_values).unwrap()
    }

    #[test]
    fn test_single_series_constant_values() {
        let mut buffer = create_test_buffer::<1, 4096>(&[1], &[100]);

        let mut writer = GorillaWriter::new(&mut buffer);
        for _ in 0..10 {
            writer.add_samples(&[100]).unwrap();
        }

        // Read and verify
        let mut reader = GorillaReader::new(&buffer);
        for _ in 0..10 {
            let samples = reader.read_samples().unwrap();
            assert_eq!(samples, &[100]);
        }
        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_single_series_constant() {
        {
            const N: usize = 1;
            const P: usize = 5;
            let mut buffer = create_test_buffer::<N, P>(&[1], &[100]);

            let iterations = ((P * u8::BITS as usize) - 39) / N;
            assert_eq!(iterations, 1);

            let mut writer = GorillaWriter::new(&mut buffer);
            writer.add_samples(&[100]).expect("to add 100 as 1 bit");
            assert!(writer.add_samples(&[100]).is_err());

            let mut reader = GorillaReader::new(&buffer);
            assert_eq!(reader.read_samples().expect("to read back 100"), &[100]);
            assert!(reader.read_samples().is_none());
        }

        {
            const N: usize = 1;
            const P: usize = 100;
            let mut buffer = create_test_buffer::<1, P>(&[1], &[100]);

            let iterations = ((P * u8::BITS as usize) - 39) / N;

            let mut writer = GorillaWriter::new(&mut buffer);
            for _ in 0..iterations {
                writer.add_samples(&[100]).expect("to add 100 as 1 bit");
            }
            assert!(writer.add_samples(&[100]).is_err());

            let mut reader = GorillaReader::new(&buffer);
            for _ in 0..iterations {
                assert_eq!(reader.read_samples().expect("to read back 100"), &[100]);
            }
            assert!(reader.read_samples().is_none());
        }
    }

    #[test]
    fn test_single_series_incremental() {
        let mut buffer = create_test_buffer::<1, 100>(&[1], &[0]);

        let iterations = 5;

        let mut writer = GorillaWriter::new(&mut buffer);
        for i in 0..iterations {
            let sample = (i + 1) % 2;
            writer.add_samples(&[sample]).expect("to add sample");
        }
        // 8 bits for the first iteration and 3 bits for each subsequent iteration.
        assert_eq!(writer.write_offset_bits, 8 + (iterations as usize - 1) * 3);

        let mut reader = GorillaReader::new(&buffer);
        for i in 0..iterations {
            assert_eq!(
                reader.read_samples().expect("to read sample"),
                &[(i + 1) % 2]
            );
        }
        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_many_series_one_bit_constant() {
        {
            const N: usize = 2;
            const P: usize = 5;
            let mut buffer = create_test_buffer::<N, P>(&[1, 2], &[100, 100]);

            let iterations = ((P * u8::BITS as usize) - 39) / 2;
            assert_eq!(iterations, 0);

            let mut writer = GorillaWriter::new(&mut buffer);
            assert!(writer.add_samples(&[100, 100]).is_err());

            let mut reader = GorillaReader::new(&buffer);
            assert!(reader.read_samples().is_none());
        }

        {
            const N: usize = 2;
            const P: usize = 6;
            let mut buffer = create_test_buffer::<N, P>(&[1, 2], &[100, 100]);

            let iterations = ((P * u8::BITS as usize) - 39) / 2;
            assert_eq!(iterations, 4);

            let mut writer = GorillaWriter::new(&mut buffer);
            for _ in 0..iterations {
                writer.add_samples(&[100, 100]).expect("to add samples");
            }
            assert!(writer.add_samples(&[100, 100]).is_err());

            let mut reader = GorillaReader::new(&buffer);
            for _ in 0..iterations {
                assert_eq!(reader.read_samples().expect("to read samples"), &[100, 100]);
            }
            assert!(reader.read_samples().is_none());
        }
    }

    #[test]
    fn test_edge_cases() {
        let mut buffer = create_test_buffer::<1, 100>(&[1], &[0]);
        let mut writer = GorillaWriter::new(&mut buffer);

        let test_values = [
            0u32,
            u32::MAX,
            0,
            1,
            u32::MAX - 1,
            u32::MAX / 2,
            0,
            0,
            u32::MAX,
            u32::MAX,
        ];

        for value in test_values {
            writer.add_samples(&[value]).unwrap();
        }

        let mut reader = GorillaReader::new(&buffer);
        for expected in test_values {
            assert_eq!(reader.read_samples().unwrap(), &[expected]);
        }
        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_random_values() {
        let mut rng = rand::rng();
        let mut written_values = Vec::new();

        let mut buffer = create_test_buffer::<3, 4096>(&[1, 2, 3], &[1000, 2000, 3000]);

        let mut writer = GorillaWriter::new(&mut buffer);
        for _ in 0..100 {
            written_values.push([
                rng.random_range(0..=1024),
                rng.random_range(u32::MAX - 1024..=u32::MAX),
                rng.random(),
            ]);
            writer.add_samples(written_values.last().unwrap()).unwrap();
        }

        let mut reader = GorillaReader::new(&buffer);
        for expected in written_values {
            let samples = reader.read_samples().unwrap();
            assert_eq!(samples, expected);
        }
        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_fixup_not_enough_ids() {
        let mut buffer = create_test_buffer::<2, 100>(&[1, 2], &[100, 200]);

        let mut writer = GorillaWriter::new(&mut buffer);

        for i in 0..10 {
            writer.add_samples(&[i, 2 * i]).expect("to add samples");
        }

        let series_ids = SeriesIdSlice::new(&[3]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_err());

        for i in 10..20 {
            writer.add_samples(&[i, 2 * i]).expect("to add samples");
        }

        let mut reader = GorillaReader::new(&buffer);
        for i in 0..20 {
            assert_eq!(reader.read_samples().expect("to add samples"), &[i, 2 * i]);
        }
        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_fixup_preorder() {
        let mut buffer = create_test_buffer::<3, 4096>(&[1], &[0]);

        let mut writer = GorillaWriter::new(&mut buffer);

        for i in 0..10 {
            writer.add_samples(&[(i + 1) % 2]).expect("to add samples");
        }

        let series_ids = SeriesIdSlice::new(&[2, 3]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_ok());

        let mut reader = GorillaReader::new(&buffer);
        for i in 0..10 {
            assert_eq!(
                reader.read_samples().expect("to add samples"),
                &[(i + 1) % 2, 0, 0]
            );
        }

        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_fixup_inorder() {
        let mut buffer = create_test_buffer::<3, 4096>(&[2], &[0]);

        let mut writer = GorillaWriter::new(&mut buffer);

        for i in 0..10 {
            writer.add_samples(&[(i + 1) % 2]).expect("to add samples");
        }

        let series_ids = SeriesIdSlice::new(&[1, 3]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_ok());

        let mut reader = GorillaReader::new(&buffer);
        for i in 0..10 {
            assert_eq!(
                reader.read_samples().expect("to add samples"),
                &[0, (i + 1) % 2, 0]
            );
        }

        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_fixup_inorder_random() {
        // let mut rng = rand::rng();
        let mut written_values = Vec::new();

        let mut buffer = create_test_buffer::<2, 32>(&[2], &[0]);

        let mut writer = GorillaWriter::new(&mut buffer);
        for i in 0..2 {
            let curr_values = &[i];
            writer.add_samples(curr_values).unwrap();
            written_values.push([0, curr_values[0]]);
        }

        let series_ids = SeriesIdSlice::new(&[1]).unwrap();
        assert!(writer.add_series_ids(series_ids.clone()).is_ok());

        for i in 0..1 {
            let curr_values = [i, i + 1];
            writer.add_samples(&curr_values).unwrap();
            written_values.push(curr_values);
        }

        let mut reader = GorillaReader::new(&buffer);
        for expected in written_values {
            let samples = reader.read_samples().unwrap();
            assert_eq!(samples, expected);
        }
        assert!(reader.read_samples().is_none());
    }

    // #[test]
    // fn test_fixup_inorder_random() {
    //     // let mut rng = rand::rng();
    //     let mut written_values = Vec::new();

    //     let mut buffer = create_test_buffer::<3, 4096>(&[2], &[1000]);

    //     let mut writer = GorillaWriter::new(&mut buffer);
    //     for i in 0..2 {
    //         let curr_values = &[i];
    //         writer.add_samples(curr_values).unwrap();
    //         written_values.push([0, curr_values[0], 0]);
    //     }

    //     let series_ids = SeriesIdSlice::new(&[1, 3]).unwrap();
    //     assert!(writer.add_series_ids(series_ids).is_ok());

    //     for i in 0..1 {
    //         let curr_values = [i, i + 1, i + 2];
    //         writer.add_samples(&curr_values).unwrap();
    //         written_values.push(curr_values);
    //     }

    //     let mut reader = GorillaReader::new(&buffer);
    //     for expected in written_values {
    //         let samples = reader.read_samples().unwrap();
    //         // assert_eq!(samples, expected);
    //     }
    //     assert!(reader.read_samples().is_none());
    // }

    #[test]
    fn test_fixup_postorder() {
        let mut buffer = create_test_buffer::<3, 4096>(&[3], &[0]);

        let mut writer = GorillaWriter::new(&mut buffer);

        for i in 0..10 {
            writer.add_samples(&[(i + 1) % 2]).expect("to add samples");
        }

        let series_ids = SeriesIdSlice::new(&[1, 2]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_ok());

        let mut reader = GorillaReader::new(&buffer);
        for i in 0..10 {
            assert_eq!(
                reader.read_samples().expect("to add samples"),
                &[0, 0, (i + 1) % 2]
            );
        }

        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_fixup_no_space() {
        let mut buffer = create_test_buffer::<2, 10>(&[1], &[0]);

        let mut writer = GorillaWriter::new(&mut buffer);

        // we have 80 bits in the payload, after adding 41 bits we will have
        // 39 bits available and the gorilla writer will refuse to add more
        // values. At that point we want to add a new series_id and make sure
        for _ in 0..41 {
            writer.add_samples(&[0]).expect("to add samples");
        }
        assert!(writer.add_samples(&[0]).is_err());

        let series_ids = SeriesIdSlice::new(&[2]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_err());
    }

    #[test]
    fn test_fixup_exact() {
        const P: usize = 10;
        let mut buffer = create_test_buffer::<2, P>(&[1], &[0]);
        let mut writer = GorillaWriter::new(&mut buffer);

        for i in [1, 3, 5, 7, 11] {
            assert!(writer.add_samples(&[i]).is_ok());
        }
        assert!(writer.add_samples(&[0]).is_err());

        // We inserted 5 numbers, we will need to insert 5 zeroes for a new series id
        // The written payload is [0, 47) we have 41 rest bits, so this should succeed

        let series_ids = SeriesIdSlice::new(&[2]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_ok());

        let mut reader = GorillaReader::new(&buffer);
        for i in [1, 3, 5, 7, 11] {
            assert_eq!(reader.read_samples().expect("to read samples"), &[i, 0]);
        }
        assert!(reader.read_samples().is_none());
    }

    #[test]
    fn test_fixup_with_new_values() {
        const P: usize = 100;
        let mut buffer = create_test_buffer::<2, P>(&[1], &[0]);
        let mut writer = GorillaWriter::new(&mut buffer);

        for i in [1, 3, 5, 7, 11] {
            assert!(writer.add_samples(&[i]).is_ok());
        }
        // assert!(writer.add_samples(&[0]).is_err());

        // We inserted 5 numbers, we will need to insert 5 zeroes for a new series id
        // The written payload is [0, 47) we have 41 rest bits, so this should succeed

        let series_ids = SeriesIdSlice::new(&[2]).unwrap();
        assert!(writer.add_series_ids(series_ids).is_ok());

        assert!(writer.add_samples(&[66, 77]).is_ok());

        let mut reader = GorillaReader::new(&buffer);
        for _ in [1, 3, 5, 7, 11] {
            let _values = reader.read_samples().expect("to read samples");
        }
        let _values = reader.read_samples().expect("to read samples");

        // for i in [1, 3, 5, 7, 11] {
        //     assert!(writer.add_samples(&[i, i]).is_ok());
        // }
        // assert!(writer.add_samples(&[0]).is_err());

        // let mut reader = GorillaReader::new(&buffer);
        // for i in [1, 3, 5, 7, 11] {
        //     let values = reader.read_samples().expect("to read samples");
        // }
        // for i in [1, 3, 5, 7, 11] {
        //     let values = reader.read_samples().expect("to read samples");
        // }
        // assert!(reader.read_samples().is_none());
    }
}
