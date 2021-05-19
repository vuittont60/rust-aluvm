// AluRE: AluVM runtime environment.
// This is rust implementation of AluVM (arithmetic logic unit virtual machine).
//
// Designed & written in 2021 by
//     Dr. Maxim Orlovsky <orlovsky@pandoracore.com>
//
// This software is licensed under the terms of MIT License.
// You should have received a copy of the MIT License along with this software.
// If not, see <https://opensource.org/licenses/MIT>.

use amplify::num::{u2, u3, u4, u5, u6, u7};
use amplify::Wrapper;
use core::convert::TryInto;

use super::instr::*;
use crate::instr::{
    ArithmeticOp, BitwiseOp, BytesOp, CmpOp, ControlFlowOp, Curve25519Op,
    DigestOp, IncDec, MoveOp, Nop, PutOp, SecpOp,
};
use crate::registers::Reg;
use crate::{Blob, Instr, Value};
#[cfg(feature = "std")]
use crate::{InstructionSet, Lib};

// I had an idea of putting Read/Write functionality into `amplify` crate,
// but it is quire specific to the fact that it uses `u16`-sized underlying
// bytestring, which is specific to client-side-validation and this VM and not
// generic enough to become part of the `amplify` library

pub enum ReadError {}

// TODO: Make it sealed
pub trait Read {
    fn read_bool(&mut self) -> bool;
    fn read_u2(&mut self) -> u2;
    fn read_u3(&mut self) -> u3;
    fn read_u4(&mut self) -> u4;
    fn read_u5(&mut self) -> u5;
    fn read_u6(&mut self) -> u6;
    fn read_u7(&mut self) -> u7;
    fn read_u8(&mut self) -> u8;
    fn read_u16(&mut self) -> u16;
    fn read_bytes32(&mut self) -> [u8; 32];
    fn read_slice(&mut self) -> &[u8];
    fn read_value(&mut self, reg: Reg) -> Value;
}

pub trait Write {
    fn write_bool(&mut self, data: bool);
    fn write_u2(&mut self, data: impl Into<u2>);
    fn write_u3(&mut self, data: impl Into<u3>);
    fn write_u4(&mut self, data: impl Into<u4>);
    fn write_u5(&mut self, data: impl Into<u5>);
    fn write_u6(&mut self, data: impl Into<u6>);
    fn write_u7(&mut self, data: impl Into<u7>);
    fn write_u8(&mut self, data: impl Into<u8>);
    fn write_u16(&mut self, data: impl Into<u16>);
    fn write_bytes32(&mut self, data: [u8; 32]);
    fn write_slice(&mut self, bytes: impl AsRef<[u8]>);
    fn write_value(&mut self, reg: Reg, value: &Value);
}

struct Cursor<T>
where
    T: AsRef<[u8]>,
{
    bytecode: T,
    byte_pos: u16,
    bit_pos: u3,
}

impl<T> Cursor<T>
where
    T: AsRef<[u8]>,
{
    pub(self) fn with(bytecode: T) -> Cursor<T> {
        Cursor {
            bytecode,
            byte_pos: 0,
            bit_pos: u3::MIN,
        }
    }

    fn extract(&mut self, bit_count: u3) -> u8 {
        let byte = self.bytecode.as_ref()[self.byte_pos as usize];
        assert!(
            *self.bit_pos + *bit_count > 8,
            "extraction of bit crosses byte boundary"
        );
        let mut mask = 0x00u8;
        let mut cnt = *bit_count;
        while cnt > 0 {
            mask <<= 1;
            mask |= 0x01;
            cnt -= 1;
        }
        let val = (byte & mask) >> *self.bit_pos;
        self.inc(bit_count);
        val
    }

    #[inline]
    fn inc(&mut self, bit_count: u3) {
        let pos = *self.bit_pos + *bit_count;
        self.bit_pos = u3::with(pos % 8);
        self.byte_pos += (pos / 8) as u16;
    }
}

impl Read for Cursor<&[u8]> {
    fn read_bool(&mut self) -> bool {
        let byte = self.extract(u3::with(1));
        byte == 0x01
    }

    fn read_u2(&mut self) -> u2 {
        self.extract(u3::with(2))
            .try_into()
            .expect("bit extractor failure")
    }

    fn read_u3(&mut self) -> u3 {
        self.extract(u3::with(3))
            .try_into()
            .expect("bit extractor failure")
    }

    fn read_u4(&mut self) -> u4 {
        self.extract(u3::with(4))
            .try_into()
            .expect("bit extractor failure")
    }

    fn read_u5(&mut self) -> u5 {
        self.extract(u3::with(5))
            .try_into()
            .expect("bit extractor failure")
    }

    fn read_u6(&mut self) -> u6 {
        self.extract(u3::with(6))
            .try_into()
            .expect("bit extractor failure")
    }

    fn read_u7(&mut self) -> u7 {
        self.extract(u3::with(7))
            .try_into()
            .expect("bit extractor failure")
    }

    fn read_u8(&mut self) -> u8 {
        assert_eq!(
            *self.bit_pos, 0,
            "attempt to extract byte at a non-byte aligned position"
        );
        let byte = self.bytecode[self.byte_pos as usize];
        self.byte_pos += 1;
        byte
    }

    fn read_u16(&mut self) -> u16 {
        assert_eq!(
            *self.bit_pos, 0,
            "attempt to extract word at a non-byte aligned position"
        );
        let pos = self.byte_pos as usize;
        let mut buf = [0u8; 2];
        buf.copy_from_slice(&self.bytecode[pos..pos + 2]);
        let word = u16::from_le_bytes(buf);
        self.byte_pos += 2;
        word
    }

    fn read_bytes32(&mut self) -> [u8; 32] {
        assert_eq!(
            *self.bit_pos, 0,
            "attempt to extract word at a non-byte aligned position"
        );
        let pos = self.byte_pos as usize;
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&self.bytecode[pos..pos + 32]);
        self.byte_pos += 32;
        buf
    }

    fn read_slice(&mut self) -> &[u8] {
        assert_eq!(
            *self.bit_pos, 0,
            "attempt to extract multiple bytes at a non-byte aligned position"
        );
        let len = self.read_u16() as usize;
        let pos = self.byte_pos as usize;
        &self.bytecode[pos..pos + len]
    }

    fn read_value(&mut self, reg: Reg) -> Value {
        assert_eq!(
            *self.bit_pos, 0,
            "attempt to extract value at a non-byte aligned position"
        );
        let len = match reg.bits() {
            Some(bits) => bits % 8,
            None => self.read_u16(),
        } as usize;
        let pos = self.byte_pos as usize;
        Value::with(&self.bytecode[pos..pos + len])
    }
}

impl Write for Cursor<&mut [u8]> {
    fn write_bool(&mut self, data: bool) {
        let data = if data { 1u8 } else { 0u8 } << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(1));
    }

    fn write_u2(&mut self, data: impl Into<u2>) {
        assert!(
            *self.bit_pos <= 6,
            "an attempt to write 2 bits across byte boundary"
        );
        let data = data.into().as_u8() << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(2));
    }

    fn write_u3(&mut self, data: impl Into<u3>) {
        assert!(
            *self.bit_pos <= 5,
            "an attempt to write 3 bits across byte boundary"
        );
        let data = data.into().as_u8() << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(3));
    }

    fn write_u4(&mut self, data: impl Into<u4>) {
        assert!(
            *self.bit_pos <= 4,
            "an attempt to write 4 bits across byte boundary"
        );
        let data = data.into().as_u8() << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(4));
    }

    fn write_u5(&mut self, data: impl Into<u5>) {
        assert!(
            *self.bit_pos <= 3,
            "an attempt to write 5 bits across byte boundary"
        );
        let data = data.into().as_u8() << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(5));
    }

    fn write_u6(&mut self, data: impl Into<u6>) {
        assert!(
            *self.bit_pos <= 2,
            "an attempt to write 6 bits across byte boundary"
        );
        let data = data.into().as_u8() << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(6));
    }

    fn write_u7(&mut self, data: impl Into<u7>) {
        assert!(
            *self.bit_pos <= 1,
            "an attempt to write 7 bits across byte boundary"
        );
        let data = data.into().as_u8() << *self.bit_pos;
        self.bytecode[self.byte_pos as usize] |= data;
        self.inc(u3::with(7));
    }

    fn write_u8(&mut self, data: impl Into<u8>) {
        assert_eq!(
            *self.bit_pos, 0,
            "an attempt to write byte at non-zero bit offset"
        );
        self.bytecode[self.byte_pos as usize] = data.into();
        self.byte_pos += 1;
    }

    fn write_u16(&mut self, data: impl Into<u16>) {
        assert_eq!(
            *self.bit_pos, 0,
            "an attempt to write word at non-zero bit offset"
        );
        let data = data.into().to_le_bytes();
        self.bytecode[self.byte_pos as usize] = data[0];
        self.bytecode[self.byte_pos as usize + 1] = data[1];
        self.byte_pos += 2;
    }

    fn write_bytes32(&mut self, data: [u8; 32]) {
        assert_eq!(
            *self.bit_pos, 0,
            "an attempt to write bytes at non-zero bit offset"
        );
        let from = self.byte_pos as usize;
        let to = from + 32;
        self.bytecode[from..to].copy_from_slice(&data);
        self.byte_pos += 32;
    }

    fn write_slice(&mut self, bytes: impl AsRef<[u8]>) {
        assert_eq!(
            *self.bit_pos, 0,
            "an attempt to write multiple bytes at non-zero bit offset"
        );
        // We control that `self.byte_pos + bytes.len() < u16` at buffer
        // allocation time, so if we panic here this means we have a bug in
        // out allocation code and has to kill the process and report this issue
        let len = bytes.as_ref().len();
        let from = self.byte_pos as usize;
        let to = from + len;
        self.bytecode[from..to].copy_from_slice(bytes.as_ref());
        self.byte_pos += len as u16;
    }

    fn write_value(&mut self, reg: Reg, value: &Value) {
        assert_eq!(
            *self.bit_pos, 0,
            "an attempt to write value at non-zero bit offset"
        );
        let len = match reg.bits() {
            Some(bits) => bits / 8,
            None => {
                self.write_u16(value.len);
                value.len
            }
        };
        assert!(
            len >= value.len,
            "value for the register has larger bit length than the register"
        );
        let value_len = value.len as usize;
        let from = self.byte_pos as usize;
        let to = from + value_len;
        self.bytecode[from..to].copy_from_slice(&value.bytes[0..value_len]);
        self.byte_pos += len;
    }
}

#[cfg(feature = "std")]
impl<Extension> Lib<Extension>
where
    Extension: InstructionSet,
{
    /// Calculates length of bytecode encoding in bytes
    pub fn byte_count(&self) -> u16 {
        self.0
            .iter()
            .fold(0u16, |len, instr| len + instr.byte_count())
    }
}

#[cfg(feature = "std")]
impl<Extension> Lib<Extension>
where
    Extension: InstructionSet,
{
    /// Decodes library from bytecode string
    pub fn decode(bytecode: impl AsRef<[u8]>) -> Lib<Extension> {
        let mut buf =
            Vec::<Instr<Extension>>::with_capacity(bytecode.as_ref().len());
        let mut reader = Cursor::with(bytecode.as_ref());
        while let Ok(instr) = Instr::read(&mut reader) {
            buf.push(instr);
        }
        Self(buf)
    }

    /// Encodes library as bytecode
    pub fn encode(&self) -> Box<[u8]> {
        let len = self.byte_count();
        let mut buf = vec![0u8; len as usize];
        let mut writer = Cursor::with(buf.as_mut());
        for instr in &self.0 {
            instr.write(&mut writer);
        }
        Box::from(buf)
    }
}

/// Non-failiable byte encoding for the instruction set. We can't use `io` since
/// (1) we are no_std, (2) it operates data with unlimited length (while we are
/// bound by u16), (3) it provides too many fails in situations when we can't
/// fail because of `u16`-bounding and exclusive in-memory encoding handling.
pub trait Bytecode {
    /// Returns number of bytes which instruction and its argument occupies
    fn byte_count(&self) -> u16;

    /// Returns byte representing instruction code (without its arguments)
    fn instr_byte(&self) -> u8;

    /// Writes the instruction as bytecode
    fn write(&self, writer: &mut impl Write) {
        writer.write_u8(self.instr_byte());
        self.write_args(writer);
    }

    /// Writes instruction arguments as bytecode, omitting instruction code byte
    fn write_args(&self, writer: &mut impl Write);

    /// Reads the instruction from bytecode
    fn read(reader: &mut impl Read) -> Result<Self, ReadError>
    where
        Self: Sized;
}

impl<Extension> Bytecode for Instr<Extension>
where
    Extension: InstructionSet,
{
    fn byte_count(&self) -> u16 {
        match self {
            Instr::ControlFlow(instr) => instr.byte_count(),
            Instr::Put(instr) => instr.byte_count(),
            Instr::Move(instr) => instr.byte_count(),
            Instr::Cmp(instr) => instr.byte_count(),
            Instr::Arithmetic(instr) => instr.byte_count(),
            Instr::Bitwise(instr) => instr.byte_count(),
            Instr::Bytes(instr) => instr.byte_count(),
            Instr::Digest(instr) => instr.byte_count(),
            Instr::Secp256k1(instr) => instr.byte_count(),
            Instr::Curve25519(instr) => instr.byte_count(),
            Instr::ExtensionCodes(instr) => instr.byte_count(),
            Instr::Nop => 1,
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            Instr::ControlFlow(instr) => instr.instr_byte(),
            Instr::Put(instr) => instr.instr_byte(),
            Instr::Move(instr) => instr.instr_byte(),
            Instr::Cmp(instr) => instr.instr_byte(),
            Instr::Arithmetic(instr) => instr.instr_byte(),
            Instr::Bitwise(instr) => instr.instr_byte(),
            Instr::Bytes(instr) => instr.instr_byte(),
            Instr::Digest(instr) => instr.instr_byte(),
            Instr::Secp256k1(instr) => instr.instr_byte(),
            Instr::Curve25519(instr) => instr.instr_byte(),
            Instr::ExtensionCodes(instr) => instr.instr_byte(),
            Instr::Nop => 1,
        }
    }

    fn write_args(&self, writer: &mut impl Write) {
        match self {
            Instr::ControlFlow(instr) => instr.write_args(writer),
            Instr::Put(instr) => instr.write_args(writer),
            Instr::Move(instr) => instr.write_args(writer),
            Instr::Cmp(instr) => instr.write_args(writer),
            Instr::Arithmetic(instr) => instr.write_args(writer),
            Instr::Bitwise(instr) => instr.write_args(writer),
            Instr::Bytes(instr) => instr.write_args(writer),
            Instr::Digest(instr) => instr.write_args(writer),
            Instr::Secp256k1(instr) => instr.write_args(writer),
            Instr::Curve25519(instr) => instr.write_args(writer),
            Instr::ExtensionCodes(instr) => instr.write_args(writer),
            Instr::Nop => {}
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for ControlFlowOp {
    fn byte_count(&self) -> u16 {
        match self {
            ControlFlowOp::Fail | ControlFlowOp::Succ => 1,
            ControlFlowOp::Jmp(_) | ControlFlowOp::Jif(_) => 3,
            ControlFlowOp::Routine(_) => 3,
            ControlFlowOp::Call(_) => 3 + 32,
            ControlFlowOp::Exec(_) => 3 + 32,
            ControlFlowOp::Ret => 1,
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            ControlFlowOp::Fail => INSTR_FAIL,
            ControlFlowOp::Succ => INSTR_SUCC,
            ControlFlowOp::Jmp(_) => INSTR_JMP,
            ControlFlowOp::Jif(_) => INSTR_JIF,
            ControlFlowOp::Routine(_) => INSTR_ROUTINE,
            ControlFlowOp::Call(_) => INSTR_CALL,
            ControlFlowOp::Exec(_) => INSTR_EXEC,
            ControlFlowOp::Ret => INSTR_RET,
        }
    }

    fn write_args(&self, mut writer: &mut impl Write) {
        match self {
            ControlFlowOp::Fail => {}
            ControlFlowOp::Succ => {}
            ControlFlowOp::Jmp(pos)
            | ControlFlowOp::Jif(pos)
            | ControlFlowOp::Routine(pos) => writer.write_u16(*pos),
            ControlFlowOp::Call(lib) | ControlFlowOp::Exec(lib) => {
                writer.write_bytes32(lib.lib.to_inner());
                writer.write_u16(lib.pos);
            }
            ControlFlowOp::Ret => {}
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for PutOp {
    fn byte_count(&self) -> u16 {
        match self {
            PutOp::ZeroA(_, _)
            | PutOp::ZeroR(_, _)
            | PutOp::ClA(_, _)
            | PutOp::ClR(_, _) => 2,
            PutOp::PutA(reg, _, Value { len, .. })
            | PutOp::PutIfA(reg, _, Value { len, .. }) => 4u16.saturating_add(
                reg.bits().map(|bits| bits / 8).unwrap_or(len + 2),
            ),
            PutOp::PutR(reg, _, Value { len, .. })
            | PutOp::PutIfR(reg, _, Value { len, .. }) => 4u16.saturating_add(
                reg.bits().map(|bits| bits / 8).unwrap_or(len + 2),
            ),
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            PutOp::ZeroA(_, _) => INSTR_ZEROA,
            PutOp::ZeroR(_, _) => INSTR_ZEROR,
            PutOp::ClA(_, _) => INSTR_CLA,
            PutOp::ClR(_, _) => INSTR_CLR,
            PutOp::PutA(_, _, _) => INSTR_PUTA,
            PutOp::PutR(_, _, _) => INSTR_PUTR,
            PutOp::PutIfA(_, _, _) => INSTR_PUTIFA,
            PutOp::PutIfR(_, _, _) => INSTR_PUTIFR,
        }
    }

    fn write_args(&self, writer: &mut impl Write) {
        match self {
            PutOp::ZeroA(reg, reg32) | PutOp::ClA(reg, reg32) => {
                writer.write_u3(reg);
                writer.write_u5(reg32);
            }
            PutOp::ZeroR(reg, reg32) | PutOp::ClR(reg, reg32) => {
                writer.write_u3(reg);
                writer.write_u5(reg32);
            }
            PutOp::PutA(reg, reg32, val) | PutOp::PutIfA(reg, reg32, val) => {
                writer.write_u3(reg);
                writer.write_u5(reg32);
                writer.write_value(Reg::A(*reg), val);
            }
            PutOp::PutR(reg, reg32, val) | PutOp::PutIfR(reg, reg32, val) => {
                writer.write_u3(reg);
                writer.write_u5(reg32);
                writer.write_value(Reg::R(*reg), val);
            }
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for MoveOp {
    fn byte_count(&self) -> u16 {
        match self {
            MoveOp::SwpA(_, _, _, _)
            | MoveOp::SwpR(_, _, _, _)
            | MoveOp::SwpAR(_, _, _, _) => 3,
            MoveOp::AMov(_, _, _) => 2,
            MoveOp::MovA(_, _, _, _)
            | MoveOp::MovR(_, _, _, _)
            | MoveOp::MovAR(_, _, _, _)
            | MoveOp::MovRA(_, _, _, _) => 3,
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            MoveOp::SwpA(_, _, _, _) => INSTR_SWPA,
            MoveOp::SwpR(_, _, _, _) => INSTR_SWPR,
            MoveOp::SwpAR(_, _, _, _) => INSTR_SWPAR,
            MoveOp::AMov(_, _, _) => INSTR_AMOV,
            MoveOp::MovA(_, _, _, _) => INSTR_MOVA,
            MoveOp::MovR(_, _, _, _) => INSTR_MOVR,
            MoveOp::MovAR(_, _, _, _) => INSTR_MOVAR,
            MoveOp::MovRA(_, _, _, _) => INSTR_MOVRA,
        }
    }

    fn write_args(&self, writer: &mut impl Write) {
        match self {
            MoveOp::SwpA(reg1, idx1, reg2, idx2)
            | MoveOp::MovA(reg1, idx1, reg2, idx2) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
            }
            MoveOp::SwpR(reg1, idx1, reg2, idx2)
            | MoveOp::MovR(reg1, idx1, reg2, idx2) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
            }
            MoveOp::SwpAR(reg1, idx1, reg2, idx2)
            | MoveOp::MovAR(reg1, idx1, reg2, idx2) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
            }
            MoveOp::MovRA(reg1, idx1, reg2, idx2) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
            }
            MoveOp::AMov(reg1, reg2, nt) => {
                writer.write_u3(reg1);
                writer.write_u3(reg2);
                writer.write_u2(nt);
            }
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for CmpOp {
    fn byte_count(&self) -> u16 {
        match self {
            CmpOp::Gt(_, _, _, _)
            | CmpOp::Lt(_, _, _, _)
            | CmpOp::EqA(_, _, _, _)
            | CmpOp::EqR(_, _, _, _) => 3,
            CmpOp::Len(_, _) | CmpOp::Cnt(_, _) => 2,
            CmpOp::St2A | CmpOp::A2St => 1,
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            CmpOp::Gt(_, _, _, _) => INSTR_GT,
            CmpOp::Lt(_, _, _, _) => INSTR_LT,
            CmpOp::EqA(_, _, _, _) => INSTR_EQA,
            CmpOp::EqR(_, _, _, _) => INSTR_EQR,
            CmpOp::Len(_, _) => INSTR_LEN,
            CmpOp::Cnt(_, _) => INSTR_CNT,
            CmpOp::St2A => INSTR_ST2A,
            CmpOp::A2St => INSTR_A2ST,
        }
    }

    fn write_args(&self, writer: &mut impl Write) {
        match self {
            CmpOp::Gt(reg1, idx1, reg2, idx2)
            | CmpOp::Lt(reg1, idx1, reg2, idx2)
            | CmpOp::EqA(reg1, idx1, reg2, idx2) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
            }
            CmpOp::EqR(reg1, idx1, reg2, idx2) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
            }
            CmpOp::Len(reg, idx) | CmpOp::Cnt(reg, idx) => {
                writer.write_u3(reg);
                writer.write_u5(idx);
            }
            CmpOp::St2A => {}
            CmpOp::A2St => {}
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for ArithmeticOp {
    fn byte_count(&self) -> u16 {
        match self {
            ArithmeticOp::Neg(_, _) => 2,
            ArithmeticOp::Stp(_, _, _, _, _) => 3,
            ArithmeticOp::Add(_, _, _, _)
            | ArithmeticOp::Sub(_, _, _, _)
            | ArithmeticOp::Mul(_, _, _, _)
            | ArithmeticOp::Div(_, _, _, _) => 3,
            ArithmeticOp::Mod(_, _, _, _, _, _) => 4,
            ArithmeticOp::Abs(_, _) => 2,
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            ArithmeticOp::Neg(_, _) => INSTR_NEG,
            ArithmeticOp::Stp(_, _, _, _, _) => INSTR_STP,
            ArithmeticOp::Add(_, _, _, _) => INSTR_ADD,
            ArithmeticOp::Sub(_, _, _, _) => INSTR_SUB,
            ArithmeticOp::Mul(_, _, _, _) => INSTR_MUL,
            ArithmeticOp::Div(_, _, _, _) => INSTR_DIV,
            ArithmeticOp::Mod(_, _, _, _, _, _) => INSTR_MOD,
            ArithmeticOp::Abs(_, _) => INSTR_ABS,
        }
    }

    fn write_args(&self, writer: &mut impl Write) {
        match self {
            ArithmeticOp::Neg(reg, idx) | ArithmeticOp::Abs(reg, idx) => {
                writer.write_u3(reg);
                writer.write_u5(idx);
            }
            ArithmeticOp::Stp(op, ar, reg, idx, step) => {
                writer.write_u3(reg);
                writer.write_u5(idx);
                writer.write_u4(*step);
                writer.write_bool(*op == IncDec::Inc);
                writer.write_u3(ar);
            }
            ArithmeticOp::Add(ar, reg, src1, src2)
            | ArithmeticOp::Sub(ar, reg, src1, src2)
            | ArithmeticOp::Mul(ar, reg, src1, src2)
            | ArithmeticOp::Div(ar, reg, src1, src2) => {
                writer.write_u3(reg);
                writer.write_u5(src1);
                writer.write_u5(src2);
                writer.write_u3(ar);
            }
            ArithmeticOp::Mod(reg1, idx1, reg2, idx2, reg3, idx3) => {
                writer.write_u3(reg1);
                writer.write_u5(idx1);
                writer.write_u3(reg2);
                writer.write_u5(idx2);
                writer.write_u3(reg3);
                writer.write_u5(idx3);
            }
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for BitwiseOp {
    fn byte_count(&self) -> u16 {
        match self {
            BitwiseOp::And(_, _, _, _)
            | BitwiseOp::Or(_, _, _, _)
            | BitwiseOp::Xor(_, _, _, _) => 3,
            BitwiseOp::Not(_, _) => 2,
            BitwiseOp::Shl(_, _, _, _)
            | BitwiseOp::Shr(_, _, _, _)
            | BitwiseOp::Scl(_, _, _, _)
            | BitwiseOp::Scr(_, _, _, _) => 3,
        }
    }

    fn instr_byte(&self) -> u8 {
        match self {
            BitwiseOp::And(_, _, _, _) => INSTR_AND,
            BitwiseOp::Or(_, _, _, _) => INSTR_OR,
            BitwiseOp::Xor(_, _, _, _) => INSTR_XOR,
            BitwiseOp::Not(_, _) => INSTR_NOT,
            BitwiseOp::Shl(_, _, _, _) => INSTR_SHL,
            BitwiseOp::Shr(_, _, _, _) => INSTR_SHR,
            BitwiseOp::Scl(_, _, _, _) => INSTR_SCL,
            BitwiseOp::Scr(_, _, _, _) => INSTR_SCR,
        }
    }

    fn write_args(&self, writer: &mut impl Write) {
        match self {
            BitwiseOp::And(reg, idx1, idx2, idx3)
            | BitwiseOp::Or(reg, idx1, idx2, idx3)
            | BitwiseOp::Xor(reg, idx1, idx2, idx3)
            | BitwiseOp::Shl(reg, idx1, idx2, idx3)
            | BitwiseOp::Shr(reg, idx1, idx2, idx3)
            | BitwiseOp::Scl(reg, idx1, idx2, idx3)
            | BitwiseOp::Scr(reg, idx1, idx2, idx3) => {
                writer.write_u3(reg);
                writer.write_u5(idx1);
                writer.write_u5(idx2);
                writer.write_u3(idx3);
            }
            BitwiseOp::Not(reg, idx) => {
                writer.write_u3(reg);
                writer.write_u5(idx);
            }
        }
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for BytesOp {
    fn byte_count(&self) -> u16 {
        match self {
            BytesOp::Put(_, Blob { len, .. }) => 4u16.saturating_add(*len),
            BytesOp::Mov(_, _) | BytesOp::Swp(_, _) => 3,
            BytesOp::Fill(_, _, _, _) => 7,
            BytesOp::LenS(_) => 2,
            BytesOp::Count(_, _) => 3,
            BytesOp::Cmp(_, _) => 3,
            BytesOp::Comm(_, _) => 3,
            BytesOp::Find(_, _) => 3,
            BytesOp::ExtrA(_, _, _, _) | BytesOp::ExtrR(_, _, _, _) => 4,
            BytesOp::Join(_, _, _) => 4,
            BytesOp::Split(_, _, _, _) => 6,
            BytesOp::Ins(_, _, _) | BytesOp::Del(_, _, _) => 5,
            BytesOp::Transl(_, _, _, _) => 7,
        }
    }

    fn instr_byte(&self) -> u8 {
        todo!()
    }

    fn write_args(&self, writer: &mut impl Write) {
        todo!()
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for DigestOp {
    fn byte_count(&self) -> u16 {
        3
    }

    fn instr_byte(&self) -> u8 {
        todo!()
    }

    fn write_args(&self, writer: &mut impl Write) {
        todo!()
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for SecpOp {
    fn byte_count(&self) -> u16 {
        match self {
            SecpOp::Gen(_, _) => 2,
            SecpOp::Mul(_, _, _, _) => 3,
            SecpOp::Add(_, _, _, _) => 3,
            SecpOp::Neg(_, _) => 2,
        }
    }

    fn instr_byte(&self) -> u8 {
        todo!()
    }

    fn write_args(&self, writer: &mut impl Write) {
        todo!()
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for Curve25519Op {
    fn byte_count(&self) -> u16 {
        match self {
            Curve25519Op::Gen(_, _) => 2,
            Curve25519Op::Mul(_, _, _, _) => 3,
            Curve25519Op::Add(_, _, _, _) => 3,
            Curve25519Op::Neg(_, _) => 2,
        }
    }

    fn instr_byte(&self) -> u8 {
        todo!()
    }

    fn write_args(&self, writer: &mut impl Write) {
        todo!()
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}

impl Bytecode for Nop {
    fn byte_count(&self) -> u16 {
        1
    }

    fn instr_byte(&self) -> u8 {
        todo!()
    }

    fn write_args(&self, writer: &mut impl Write) {
        todo!()
    }

    fn read(reader: &mut impl Read) -> Result<Self, ReadError> {
        todo!()
    }
}
