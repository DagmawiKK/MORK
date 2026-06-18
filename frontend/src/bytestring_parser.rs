use mork_expr::{ExprZipper, Tag, item_byte, byte_item, read_arity_at};

#[allow(non_snake_case)]
fn isWhitespace(c: u8) -> bool {
  c == b' ' || c == b'\t' || c == b'\n'
}

#[allow(non_snake_case, unused)]
fn isDigit(c: u8) -> bool {
  c == b'0' || c == b'1' || c == b'2' || c == b'3' || c == b'4' ||
  c == b'5' || c == b'6' || c == b'7' || c == b'8' || c == b'9'
}

#[derive(Debug)]
pub enum ParserError {
  TooManyVars,
  UnexpectedEOF,
  InputFinished,
  NotArity,
  UnexpectedRightBracket,
  UnfinishedEscapeSequence
}

pub struct Context<'a> {
  pub src: &'a [u8],
  pub loc: usize,
  pub variables: Vec<&'a [u8]>
}

impl <'a> Context<'a> {
  pub fn new(r: &'a [u8]) -> Context<'a> {
    Context{ src: r, loc: 0, variables: vec![] }
  }

  #[inline(always)]
  fn peek(&mut self) -> Result<u8, ParserError> {
    if self.loc == self.src.len() {
      Err(ParserError::UnexpectedEOF)
    } else {
      Ok(unsafe { *self.src.get_unchecked(self.loc) })
    }
  }

  #[inline(always)]
  fn next(&mut self) -> Result<u8, ParserError> {
    if self.loc == self.src.len() {
      Err(ParserError::UnexpectedEOF)
    } else {
      let r = unsafe { *self.src.get_unchecked(self.loc) };
      self.loc += 1;
      Ok(r)
    }
  }

  #[inline(always)]
  fn has_next(&mut self) -> bool {
    self.loc < self.src.len()
  }

  #[inline]
  fn get_or_put(&mut self, var: &'a [u8]) -> Result<Option<u8>, ParserError> {
    let mut i = 0;
    for &v in self.variables.iter() {
      if var == v { return Ok(Some(i as u8)) }
      else { i += 1 }
    }

    if self.variables.len() < 128 {
      // we can only have 128 variables, we don't need a vec here, perhaps uninit array?
      self.variables.push(var);
      Ok(None)
    } else {
      Err(ParserError::TooManyVars)
    }
  }
}

pub trait Parser {
  fn tokenizer<'r>(&mut self, s: &[u8]) -> &'r [u8];

  fn sexpr<'a>(&mut self, it: &mut Context<'a>, target: &mut ExprZipper) -> Result<(), ParserError> {
    use ParserError::*;
    while it.has_next() {
      match it.peek()? {
        b';' => { while it.has_next() && it.next()? != b'\n' {} }
        c if isWhitespace(c) => { it.next()?; }
        b'$' => {
          let id = {
            let start = it.loc;
            while it.has_next() {
              match it.peek()? {
                b'(' | b')' => { break }
                c if isWhitespace(c) => { break }
                _ => { it.next()?; }
              }
            }
            unsafe { &it.src.get_unchecked(start..it.loc) }
          };
          match it.get_or_put(id)? {
            None => { target.write_new_var(); target.loc += 1; }
            Some(ind) => { target.write_var_ref(ind); target.loc += mork_expr::var_ref_byte_count_at(unsafe { target.root.ptr.byte_add(target.loc) }); }
          }
          return Ok(());
        }
        b'(' => {
          let arity_loc = target.loc;
          unsafe { *target.root.ptr.byte_add(target.loc) = 0b0000_0000; }
          target.loc += 1;
          it.next()?;
          while it.peek()? != b')' {
            match it.peek()? {
              c if isWhitespace(c) => { it.next()?; }
              _ => {
                self.sexpr(it, target)?;
                unsafe {
                  let p = target.root.ptr.byte_add(arity_loc);
                  match byte_item(*p) {
                    Tag::Arity(a) => {
                      if a < 63 {
                        *p = item_byte(Tag::Arity(a + 1));
                      } else {
                        // Expand to 2-byte LongArity: shift subsequent bytes right by 1
                        let end = target.loc;
                        let src = target.root.ptr.byte_add(arity_loc + 1);
                        let dst = target.root.ptr.byte_add(arity_loc + 2);
                        let count = end - (arity_loc + 1);
                        std::ptr::copy(src, dst, count);
                        // Write arity 64 as 2-byte LongArity
                        *p = 0b0100_0000 | 0; // low 6 bits: 64 & 0x3F = 0
                        *target.root.ptr.byte_add(arity_loc + 1) = 1;   // (64 >> 6) = 1
                        target.loc += 1;
                      }
                    }
                    Tag::LongArity => {
                      let a = read_arity_at(p);
                      let new_a = a + 1;
                      *p = 0b0100_0000 | (new_a as u8 & 0b0011_1111);
                      *target.root.ptr.byte_add(arity_loc + 1) = (new_a >> 6) as u8;
                    }
                    _ => return Err(NotArity),
                  }
                }
              }
            }
          }
          it.next()?;
          return Ok(())
        }
        b')' => { return Err(UnexpectedRightBracket) }
        _ => {
          let start = it.loc;
          if it.has_next() && it.peek()? == b'"' {
            it.next()?;
            while it.has_next() {
              match it.next()? {
                b'"' => { break }
                b'\\' => {
                  if it.has_next() { it.next()?; }
                  else { return Err(UnfinishedEscapeSequence) }
                }
                _ => {}
              }
            }
          } else {
            while it.has_next() {
              match it.peek()? {
                b'(' | b')' => { break }
                c if isWhitespace(c) => { break }
                _ => { it.next()?; }
              }
            }
          }

          let e = self.tokenizer(unsafe { &it.src.get_unchecked(start..it.loc) });
          target.write_symbol(e);
          target.loc += 1 + e.len();
          return Ok(());
        }
      }
    }
    Err(InputFinished)
  }
}
