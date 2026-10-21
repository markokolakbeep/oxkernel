/// Callee-saved registers, saved and restored by `__switch`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
}

impl Context {
    pub const fn zero() -> Self {
        Self { ra: 0, sp: 0, s: [0; 12] }
    }
}

unsafe extern "C" {
    pub fn __switch(old: *mut Context, new: *const Context);
}
