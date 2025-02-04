// SPDX-License-Identifier: MIT

//! Degree-2 [BCH] code checksum.
//!
//! [BCH]: <https://en.wikipedia.org/wiki/BCH_code>

use core::{mem, ops};

use crate::primitives::gf32::Fe32;
use crate::primitives::hrp::Hrp;

/// Trait defining a particular checksum.
///
/// For users, this can be treated as a marker trait; none of the associated data
/// are end-user relevant.
pub trait Checksum {
    /// An unsigned integer type capable of holding a packed version of the generator
    /// polynomial (without its leading 1) and target residue (which will have the
    /// same width).
    ///
    /// Generally, this is the number of characters in the checksum times 5. So e.g.
    /// for bech32, which has a 6-character checksum, we need 30 bits, so we can use
    /// u32 here.
    ///
    /// The smallest type possible should be used, for efficiency reasons, but the
    /// only operations we do on these types are bitwise xor and shifts, so it should
    /// be pretty efficient no matter what.
    type MidstateRepr: PackedFe32;

    /// The length of the code.
    ///
    /// The length of the code is how long a coded message can be (including the
    /// checksum!) for the code to retain its error-correcting properties.
    const CODE_LENGTH: usize;

    /// The number of characters in the checksum.
    ///
    /// Alternately, the degree of the generator polynomial. This is **not** the same
    /// as `Self::CODE_LENGTH`.
    const CHECKSUM_LENGTH: usize;

    /// The coefficients of the generator polynomial, except the leading monic term,
    /// in "big-endian" (highest-degree coefficients get leftmost bits) order, along
    /// with the 4 shifts of the generator.
    ///
    /// The shifts are literally the generator polynomial left-shifted (i.e. multiplied
    /// by the appropriate power of 2) in the field. That is, the 5 entries in this
    /// array are the generator times { P, Z, Y, G, S } in that order.
    ///
    /// These cannot be usefully pre-computed because of Rust's limited constfn support
    /// as of 1.67, so they must be specified manually for each checksum. To check the
    /// values for consistency, run `Self::sanity_check()`.
    const GENERATOR_SH: [Self::MidstateRepr; 5];

    /// The residue, modulo the generator polynomial, that a valid codeword will have.
    const TARGET_RESIDUE: Self::MidstateRepr;

    /// Sanity checks that the various constants of the trait are set in a way that they
    /// are consistent with each other.
    ///
    /// This function never needs to be called by users, but anyone defining a checksum
    /// should add a unit test to their codebase which calls this.
    fn sanity_check() {
        // Check that the declared midstate type can actually hold the whole checksum.
        assert!(Self::CHECKSUM_LENGTH <= Self::MidstateRepr::WIDTH);

        // Check that the provided generator polynomials are, indeed, the same polynomial just shifted.
        for i in 1..5 {
            for j in 0..Self::MidstateRepr::WIDTH {
                let last = Self::GENERATOR_SH[i - 1].unpack(j);
                let curr = Self::GENERATOR_SH[i].unpack(j);
                // GF32 is defined by extending GF2 with a root of x^5 + x^3 + 1 = 0
                // which when written as bit coefficients is 41 = 0. Hence xoring
                // (adding, in GF32) by 41 is the way to reduce x^5.
                assert_eq!(
                    curr,
                    (last << 1) ^ if last & 0x10 == 0x10 { 41 } else { 0 },
                    "Element {} of generator << 2^{} was incorrectly computed. (Should have been {} << 1)",
                    j, i, last,
                );
            }
        }
    }
}

/// A checksum engine, which can be used to compute or verify a checksum.
///
/// Use this to verify a checksum, feed it the data to be checksummed using
/// the `Self::input_*` methods.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Engine<Ck: Checksum> {
    residue: Ck::MidstateRepr,
}

impl<Ck: Checksum> Default for Engine<Ck> {
    fn default() -> Self { Self::new() }
}

impl<Ck: Checksum> Engine<Ck> {
    /// Constructs a new checksum engine with no data input.
    #[inline]
    pub fn new() -> Self { Engine { residue: Ck::MidstateRepr::ONE } }

    /// Feeds `hrp` into the checksum engine.
    #[inline]
    pub fn input_hrp(&mut self, hrp: Hrp) {
        for fe in HrpFe32Iter::new(&hrp) {
            self.input_fe(fe)
        }
    }

    /// Adds a single gf32 element to the checksum engine.
    ///
    /// This is where the actual checksum computation magic happens.
    #[inline]
    pub fn input_fe(&mut self, e: Fe32) {
        let xn = self.residue.mul_by_x_then_add(Ck::CHECKSUM_LENGTH, e.into());
        for i in 0..5 {
            if xn & (1 << i) != 0 {
                self.residue = self.residue ^ Ck::GENERATOR_SH[i];
            }
        }
    }

    /// Inputs the target residue of the checksum.
    ///
    /// Checksums are generated by appending the target residue to the input
    /// string, then computing the actual residue, and then replacing the
    /// target with the actual. This method lets us compute the actual residue
    /// without doing any string concatenations.
    #[inline]
    pub fn input_target_residue(&mut self) {
        for i in 0..Ck::CHECKSUM_LENGTH {
            self.input_fe(Fe32(Ck::TARGET_RESIDUE.unpack(Ck::CHECKSUM_LENGTH - i - 1)));
        }
    }

    /// Returns for the current checksum residue.
    #[inline]
    pub fn residue(&self) -> &Ck::MidstateRepr { &self.residue }
}

/// Trait describing an integer type which can be used as a "packed" sequence of Fe32s.
///
/// This is implemented for u32, u64 and u128, as a way to treat these primitive types as
/// packed coefficients of polynomials over GF32 (up to some maximal degree, of course).
///
/// This is useful because then multiplication by x reduces to simply left-shifting by 5,
/// and addition of entire polynomials can be done by xor.
pub trait PackedFe32: Copy + PartialEq + Eq + ops::BitXor<Self, Output = Self> {
    /// The one constant, for which stdlib provides no existing trait.
    const ONE: Self;

    /// The number of fe32s that can fit into the type; computed as floor(bitwidth / 5).
    const WIDTH: usize = mem::size_of::<Self>() * 8 / 5;

    /// Extracts the coefficient of the x^n from the packed polynomial.
    fn unpack(&self, n: usize) -> u8;

    /// Multiply the polynomial by x, drop its highest coefficient (and return it), and
    /// add a new field element to the now-0 constant coefficient.
    ///
    /// Takes the degree of the polynomial as an input; for checksum applications
    /// this should basically always be `Checksum::CHECKSUM_WIDTH`.
    fn mul_by_x_then_add(&mut self, degree: usize, add: u8) -> u8;
}

/// A placeholder type used as part of the [`crate::primitives::NoChecksum`] "checksum".
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PackedNull;

impl ops::BitXor<PackedNull> for PackedNull {
    type Output = PackedNull;
    #[inline]
    fn bitxor(self, _: PackedNull) -> PackedNull { PackedNull }
}

impl PackedFe32 for PackedNull {
    const ONE: Self = PackedNull;
    #[inline]
    fn unpack(&self, _: usize) -> u8 { 0 }
    #[inline]
    fn mul_by_x_then_add(&mut self, _: usize, _: u8) -> u8 { 0 }
}

macro_rules! impl_packed_fe32 {
    ($ty:ident) => {
        impl PackedFe32 for $ty {
            const ONE: Self = 1;

            #[inline]
            fn unpack(&self, n: usize) -> u8 {
                debug_assert!(n < Self::WIDTH);
                (*self >> (n * 5)) as u8 & 0x1f
            }

            #[inline]
            fn mul_by_x_then_add(&mut self, degree: usize, add: u8) -> u8 {
                debug_assert!(degree > 0);
                debug_assert!(degree <= Self::WIDTH);
                debug_assert!(add < 32);
                let ret = self.unpack(degree - 1);
                *self &= !(0x1f << ((degree - 1) * 5));
                *self <<= 5;
                *self |= Self::from(add);
                ret
            }
        }
    };
}
impl_packed_fe32!(u32);
impl_packed_fe32!(u64);
impl_packed_fe32!(u128);

/// Iterator that yields the field elements that are input into a checksum algorithm for an [`Hrp`].
pub struct HrpFe32Iter<'hrp> {
    /// `None` once the hrp high fes have been yielded.
    high_iter: Option<crate::primitives::hrp::LowercaseByteIter<'hrp>>,
    /// `None` once the hrp low fes have been yielded.
    low_iter: Option<crate::primitives::hrp::LowercaseByteIter<'hrp>>,
}

impl<'hrp> HrpFe32Iter<'hrp> {
    /// Creates an iterator that yields the field elements of `hrp` as they are input into the
    /// checksum algorithm.
    #[inline]
    pub fn new(hrp: &'hrp Hrp) -> Self {
        let high_iter = hrp.lowercase_byte_iter();
        let low_iter = hrp.lowercase_byte_iter();

        Self { high_iter: Some(high_iter), low_iter: Some(low_iter) }
    }
}

impl<'hrp> Iterator for HrpFe32Iter<'hrp> {
    type Item = Fe32;
    #[inline]
    fn next(&mut self) -> Option<Fe32> {
        if let Some(ref mut high_iter) = &mut self.high_iter {
            match high_iter.next() {
                Some(high) => return Some(Fe32(high >> 5)),
                None => {
                    self.high_iter = None;
                    return Some(Fe32::Q);
                }
            }
        }
        if let Some(ref mut low_iter) = &mut self.low_iter {
            match low_iter.next() {
                Some(low) => return Some(Fe32(low & 0x1f)),
                None => self.low_iter = None,
            }
        }
        None
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let high = match &self.high_iter {
            Some(high_iter) => {
                let (min, max) = high_iter.size_hint();
                (min + 1, max.map(|max| max + 1)) // +1 for the extra Q
            }
            None => (0, Some(0)),
        };
        let low = match &self.low_iter {
            Some(low_iter) => low_iter.size_hint(),
            None => (0, Some(0)),
        };

        let min = high.0 + 1 + low.0;
        let max = high.1.zip(low.1).map(|(high, low)| high + 1 + low);

        (min, max)
    }
}
