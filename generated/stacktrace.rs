extern crate libcc2rs;
use libcc2rs::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::prelude::*;
use std::io::{Read, Seek, Write};
use std::os::fd::AsFd;
use std::rc::{Rc, Weak};
thread_local!(
    pub static global_progname_0: Value<Ptr<u8>> = Rc::new(RefCell::new(Ptr::<u8>::null()));
);
thread_local!(
    pub static global_output_1: Value<i32> = Rc::new(RefCell::new(1));
);
pub fn stacktrace_format_command_2(
    dst: Ptr<u8>,
    dst_size: usize,
    format: Ptr<u8>,
    __args: &[VaArg],
) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let format: Value<Ptr<u8>> = Rc::new(RefCell::new(format));
    let written: Value<i32> = <Value<i32>>::default();
    let args: Value<VaList> = Rc::new(RefCell::new(VaList::default()));
    if ((((((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
        || ((((*format.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    (*args.borrow_mut()) = VaList::new(__args);
    (*written.borrow_mut()) = ({
        vsnprintf_3(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (*format.borrow()).clone(),
            (*args.borrow()).clone(),
        )
    });
    if (((((((*written.borrow()) < 0) as i32) != 0)
        || (((((*written.borrow()) as usize) >= (*dst_size.borrow())) as i32) != 0))
        as i32)
        != 0)
    {
        (*dst.borrow())
            .offset(((*dst_size.borrow()).wrapping_sub(1_usize)) as isize)
            .write((0 as u8));
        return -1_i32;
    }
    return 0;
}
pub fn stacktrace_append_text_4(
    dst: Ptr<u8>,
    dst_size: usize,
    pos: Ptr<usize>,
    src: Ptr<u8>,
) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let pos: Value<Ptr<usize>> = Rc::new(RefCell::new(pos));
    let src: Value<Ptr<u8>> = Rc::new(RefCell::new(src));
    let i: Value<usize> = Rc::new(RefCell::new(0_usize));
    if (((((((((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
        || ((((*pos.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
        || ((((*src.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    (*i.borrow_mut()) = 0_usize;
    'loop_: while ((((((*src.borrow()).offset((*i.borrow()) as isize).read()) as i32)
        != ('\0' as i32)) as i32)
        != 0)
    {
        if ((({
            let _lhs = ((*pos.borrow()).read()).wrapping_add(1_usize);
            _lhs >= (*dst_size.borrow())
        }) as i32)
            != 0)
        {
            (*dst.borrow())
                .offset(((*dst_size.borrow()).wrapping_sub(1_usize)) as isize)
                .write((('\0' as i32) as u8));
            return -1_i32;
        }
        let __rhs = ((*src.borrow()).offset((*i.borrow()) as isize).read());
        (*dst.borrow())
            .offset(((*pos.borrow()).read()) as isize)
            .write(__rhs);
        (*pos.borrow()).with_mut(|__v| __v.postfix_inc());
        (*i.borrow_mut()).postfix_inc();
    }
    (*dst.borrow())
        .offset(((*pos.borrow()).read()) as isize)
        .write((('\0' as i32) as u8));
    return 0;
}
pub fn stacktrace_append_shell_quoted_5(
    dst: Ptr<u8>,
    dst_size: usize,
    pos: Ptr<usize>,
    src: Ptr<u8>,
) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let pos: Value<Ptr<usize>> = Rc::new(RefCell::new(pos));
    let src: Value<Ptr<u8>> = Rc::new(RefCell::new(src));
    thread_local!(
        static single_quote_escape_6: Value<Box<[u8]>> =
            Rc::new(RefCell::new(Box::from(*b"\'\"\'\"\'\0")));
    );
    if (((((((((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
        || ((((*pos.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
        || ((((*src.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    if (((({
        let _dst_size: usize = (*dst_size.borrow());
        let _pos: Ptr<usize> = (*pos.borrow()).clone();
        stacktrace_append_text_4(
            (*dst.borrow()).clone(),
            _dst_size,
            _pos,
            Ptr::from_string_literal(b"\'"),
        )
    }) != 0) as i32)
        != 0)
    {
        return -1_i32;
    }
    'loop_: while ((((((*src.borrow()).read()) as i32) != ('\0' as i32)) as i32) != 0) {
        if ((((((*src.borrow()).read()) as i32) == ('\'' as i32)) as i32) != 0) {
            if (((({
                let _dst_size: usize = (*dst_size.borrow());
                let _pos: Ptr<usize> = (*pos.borrow()).clone();
                stacktrace_append_text_4(
                    (*dst.borrow()).clone(),
                    _dst_size,
                    _pos,
                    (single_quote_escape_6.with(Value::clone).as_pointer() as Ptr<u8>),
                )
            }) != 0) as i32)
                != 0)
            {
                return -1_i32;
            }
        } else {
            let ch: Value<Box<[u8]>> = Rc::new(RefCell::new(
                (0..2).map(|_| <u8>::default()).collect::<Box<[u8]>>(),
            ));
            let __rhs = ((*src.borrow()).read());
            (*ch.borrow_mut())[(0) as usize] = __rhs;
            (*ch.borrow_mut())[(1) as usize] = (('\0' as i32) as u8);
            if (((({
                let _dst_size: usize = (*dst_size.borrow());
                let _pos: Ptr<usize> = (*pos.borrow()).clone();
                stacktrace_append_text_4(
                    (*dst.borrow()).clone(),
                    _dst_size,
                    _pos,
                    (ch.as_pointer() as Ptr<u8>),
                )
            }) != 0) as i32)
                != 0)
            {
                return -1_i32;
            }
        }
        (*src.borrow_mut()).postfix_inc();
    }
    return ({
        let _dst_size: usize = (*dst_size.borrow());
        let _pos: Ptr<usize> = (*pos.borrow()).clone();
        stacktrace_append_text_4(
            (*dst.borrow()).clone(),
            _dst_size,
            _pos,
            Ptr::from_string_literal(b"\'"),
        )
    });
}
pub fn stacktrace_shell_quote_7(dst: Ptr<u8>, dst_size: usize, src: Ptr<u8>) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let src: Value<Ptr<u8>> = Rc::new(RefCell::new(src));
    let pos: Value<usize> = Rc::new(RefCell::new(0_usize));
    if (((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    (*dst.borrow())
        .offset((0) as isize)
        .write((('\0' as i32) as u8));
    return ({
        stacktrace_append_shell_quoted_5(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            (*src.borrow()).clone(),
        )
    });
}
pub fn stacktrace_parse_nm_symbol_line_8(
    line: Ptr<u8>,
    addr: Ptr<u64>,
    type_: Ptr<u8>,
    name: Ptr<u8>,
    name_size: usize,
) -> i32 {
    let line: Value<Ptr<u8>> = Rc::new(RefCell::new(line));
    let addr: Value<Ptr<u64>> = Rc::new(RefCell::new(addr));
    let type_: Value<Ptr<u8>> = Rc::new(RefCell::new(type_));
    let name: Value<Ptr<u8>> = Rc::new(RefCell::new(name));
    let name_size: Value<usize> = Rc::new(RefCell::new(name_size));
    let offset: Value<i32> = Rc::new(RefCell::new(0));
    let i: Value<usize> = Rc::new(RefCell::new(0_usize));
    if ((((((((((((((((*line.borrow()).is_null()) as i32) != 0)
        || ((((*addr.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
        || ((((*type_.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
        || ((((*name.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
        || ((((*name_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
    {
        return 0;
    }
    (*name.borrow())
        .offset((0) as isize)
        .write((('\0' as i32) as u8));
    if (((({
        sscanf_9(
            (*line.borrow()).clone(),
            Ptr::from_string_literal(b"%lx %c %n"),
            &[
                ((*addr.borrow()).clone()).into(),
                ((*type_.borrow()).clone()).into(),
                (offset.as_pointer()).into(),
            ],
        )
    }) != 2) as i32)
        != 0)
    {
        return 0;
    }
    'loop_: while (((((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
        == (' ' as i32)) as i32)
        != 0)
        || ((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
            == ('\t' as i32)) as i32)
            != 0)) as i32)
        != 0)
    {
        (*offset.borrow_mut()).postfix_inc();
    }
    if (((((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
        == ('\0' as i32)) as i32)
        != 0)
        || ((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
            == ('\n' as i32)) as i32)
            != 0)) as i32)
        != 0)
    {
        return 0;
    }
    'loop_: while (((((((((((((((*line.borrow()).offset((*offset.borrow()) as isize).read())
        as i32)
        != ('\0' as i32)) as i32)
        != 0)
        && ((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
            != ('\n' as i32)) as i32)
            != 0)) as i32)
        != 0)
        && ((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
            != (' ' as i32)) as i32)
            != 0)) as i32)
        != 0)
        && ((((((*line.borrow()).offset((*offset.borrow()) as isize).read()) as i32)
            != ('\t' as i32)) as i32)
            != 0)) as i32)
        != 0)
    {
        if ((((*i.borrow()).wrapping_add(1_usize) < (*name_size.borrow())) as i32) != 0) {
            let __rhs = ((*line.borrow()).offset((*offset.borrow()) as isize).read());
            (*name.borrow())
                .offset(((*i.borrow_mut()).postfix_inc()) as isize)
                .write(__rhs);
        }
        (*offset.borrow_mut()).postfix_inc();
    }
    (*name.borrow())
        .offset((*i.borrow()) as isize)
        .write((('\0' as i32) as u8));
    return 1;
}
pub fn stacktrace_copy_symbol_name_10(dst: Ptr<u8>, dst_size: usize, src: Ptr<u8>) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let src: Value<Ptr<u8>> = Rc::new(RefCell::new(src));
    let i: Value<usize> = Rc::new(RefCell::new(0_usize));
    if ((((((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
        || ((((*src.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    'loop_: while (((((((((*src.borrow()).offset((*i.borrow()) as isize).read()) as i32)
        != ('\0' as i32)) as i32)
        != 0)
        && ((((*i.borrow()).wrapping_add(1_usize) < (*dst_size.borrow())) as i32) != 0))
        as i32)
        != 0)
    {
        let __rhs = ((*src.borrow()).offset((*i.borrow()) as isize).read());
        (*dst.borrow()).offset((*i.borrow()) as isize).write(__rhs);
        (*i.borrow_mut()).postfix_inc();
    }
    (*dst.borrow())
        .offset((*i.borrow()) as isize)
        .write((('\0' as i32) as u8));
    return 0;
}
pub fn stacktrace_format_symbol_entry_11(
    dst: Ptr<u8>,
    dst_size: usize,
    index: i32,
    real_address: u64,
    symbol_name: Ptr<u8>,
    offset: u64,
    type_: u8,
) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let index: Value<i32> = Rc::new(RefCell::new(index));
    let real_address: Value<u64> = Rc::new(RefCell::new(real_address));
    let symbol_name: Value<Ptr<u8>> = Rc::new(RefCell::new(symbol_name));
    let offset: Value<u64> = Rc::new(RefCell::new(offset));
    let type_: Value<u8> = Rc::new(RefCell::new(type_));
    if (((((((*symbol_name.borrow()).is_null()) as i32) != 0)
        || ((((((*symbol_name.borrow()).offset((0) as isize).read()) as i32) == ('\0' as i32))
            as i32)
            != 0)) as i32)
        != 0)
    {
        return ({
            stacktrace_format_command_2(
                (*dst.borrow()).clone(),
                (*dst_size.borrow()),
                Ptr::from_string_literal(b"[%d] 0x%08lx ???\n"),
                &[(*index.borrow()).into(), (*real_address.borrow()).into()],
            )
        });
    }
    return ({
        stacktrace_format_command_2(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            Ptr::from_string_literal(b"[%d] 0x%08lx <%s + 0x%lx> %c\n"),
            &[
                (*index.borrow()).into(),
                (*real_address.borrow()).into(),
                ((*symbol_name.borrow()).clone()).into(),
                (*offset.borrow()).into(),
                ((*type_.borrow()) as i32).into(),
            ],
        )
    });
}
pub fn stacktrace_build_nm_command_12(
    dst: Ptr<u8>,
    dst_size: usize,
    use_gnu_nm: i32,
    progname: Ptr<u8>,
) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let use_gnu_nm: Value<i32> = Rc::new(RefCell::new(use_gnu_nm));
    let progname: Value<Ptr<u8>> = Rc::new(RefCell::new(progname));
    let prefix: Value<Ptr<u8>> = Rc::new(RefCell::new(Ptr::<u8>::null()));
    let pos: Value<usize> = Rc::new(RefCell::new(0_usize));
    if ((((*progname.borrow()).is_null()) as i32) != 0) {
        return -1_i32;
    }
    (*prefix.borrow_mut()) = if ((*use_gnu_nm.borrow()) != 0) {
        Ptr::from_string_literal(b"nm -B ")
    } else {
        Ptr::from_string_literal(b"nm -B ")
    };
    if (((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    (*dst.borrow())
        .offset((0) as isize)
        .write((('\0' as i32) as u8));
    if (((({
        stacktrace_append_text_4(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            (*prefix.borrow()).clone(),
        )
    }) != 0) as i32)
        != 0)
    {
        return -1_i32;
    }
    return ({
        stacktrace_append_shell_quoted_5(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            (*progname.borrow()).clone(),
        )
    });
}
pub fn stacktrace_build_debugger_command_13(
    dst: Ptr<u8>,
    dst_size: usize,
    progname: Ptr<u8>,
    gdb_command_file: Ptr<u8>,
) -> i32 {
    let dst: Value<Ptr<u8>> = Rc::new(RefCell::new(dst));
    let dst_size: Value<usize> = Rc::new(RefCell::new(dst_size));
    let progname: Value<Ptr<u8>> = Rc::new(RefCell::new(progname));
    let gdb_command_file: Value<Ptr<u8>> = Rc::new(RefCell::new(gdb_command_file));
    let pos: Value<usize> = Rc::new(RefCell::new(0_usize));
    if (((((((*progname.borrow()).is_null()) as i32) != 0)
        || ((((*gdb_command_file.borrow()).is_null()) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    if (((((((*dst.borrow()).is_null()) as i32) != 0)
        || ((((*dst_size.borrow()) == 0_usize) as i32) != 0)) as i32)
        != 0)
    {
        return -1_i32;
    }
    (*dst.borrow())
        .offset((0) as isize)
        .write((('\0' as i32) as u8));
    if (((({
        stacktrace_append_text_4(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            Ptr::from_string_literal(b"gdb -q "),
        )
    }) != 0) as i32)
        != 0)
    {
        return -1_i32;
    }
    if (((({
        stacktrace_append_shell_quoted_5(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            (*progname.borrow()).clone(),
        )
    }) != 0) as i32)
        != 0)
    {
        return -1_i32;
    }
    if (((({
        let _dst: Ptr<u8> = (*dst.borrow()).offset((*pos.borrow()) as isize);
        let _dst_size: usize = (*dst_size.borrow()).wrapping_sub((*pos.borrow()));
        stacktrace_format_command_2(
            _dst,
            _dst_size,
            Ptr::from_string_literal(b" %d 2>/dev/null <"),
            &[(({ getpid_14() }) as i32).into()],
        )
    }) != 0) as i32)
        != 0)
    {
        return -1_i32;
    }
    {
        let rhs_0 = (((*pos.borrow()) as u64).wrapping_add(
            ((*dst.borrow())
                .offset((*pos.borrow()) as isize)
                .to_string_iterator()
                .count() as u64),
        )) as usize;
        (*pos.borrow_mut()) = rhs_0
    };
    if (((({
        stacktrace_append_shell_quoted_5(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            (*gdb_command_file.borrow()).clone(),
        )
    }) != 0) as i32)
        != 0)
    {
        return -1_i32;
    }
    return ({
        stacktrace_append_text_4(
            (*dst.borrow()).clone(),
            (*dst_size.borrow()),
            (pos.as_pointer()),
            Ptr::from_string_literal(b" >fweelin-stackdump"),
        )
    });
}
pub fn my_pclose_15(fd: i32, pid: i32) {
    let fd: Value<i32> = Rc::new(RefCell::new(fd));
    let pid: Value<i32> = Rc::new(RefCell::new(pid));
    libc::close((*fd.borrow()));
    ({ kill_16((*pid.borrow()), 15) });
}
pub fn my_popen_17(command: Ptr<u8>, pid: Ptr<i32>) -> i32 {
    let command: Value<Ptr<u8>> = Rc::new(RefCell::new(command));
    let pid: Value<Ptr<i32>> = Rc::new(RefCell::new(pid));
    let rc: Value<i32> = <Value<i32>>::default();
    (*rc.borrow_mut()) = -1_i32;
    let __rhs = ({ fork_18() });
    (*pid.borrow()).write(__rhs);
    'switch: {
        let __match_cond = ((*pid.borrow()).read());
        match __match_cond {
            __v if __v == -1_i32 => {
                break 'switch;
            }
            __v if __v == 0 => {
                ({
                    execl_19(
                        Ptr::from_string_literal(b"/bin/sh"),
                        Ptr::from_string_literal(b"/bin/sh"),
                        &[
                            (Ptr::from_string_literal(b"-c")).into(),
                            ((*command.borrow()).clone()).into(),
                            (AnyPtr::default()).into(),
                        ],
                    )
                });
                ({ _exit_20(1) });
                break 'switch;
            }
            _ => {
                (*rc.borrow_mut()) = 0;
                break 'switch;
            }
        }
    };
    return (*rc.borrow());
}
pub fn my_getline_21(fd: i32, buffer: Ptr<u8>, max: i32) -> i32 {
    let fd: Value<i32> = Rc::new(RefCell::new(fd));
    let buffer: Value<Ptr<u8>> = Rc::new(RefCell::new(buffer));
    let max: Value<i32> = Rc::new(RefCell::new(max));
    let c: Value<u8> = <Value<u8>>::default();
    let i: Value<i32> = Rc::new(RefCell::new(0));
    let mut __do_while = true;
    'loop_: while __do_while || (((((*c.borrow()) as i32) != ('\n' as i32)) as i32) != 0) {
        __do_while = false;
        if (((libc::read(
            (*fd.borrow()),
            ((c.as_pointer()) as Ptr<u8>).to_any(),
            1_usize,
        ) < 1_isize) as i32)
            != 0)
        {
            return 0;
        }
        if ((((*i.borrow()) < (*max.borrow())) as i32) != 0) {
            let __rhs = (*c.borrow());
            (*buffer.borrow())
                .offset(((*i.borrow_mut()).postfix_inc()) as isize)
                .write(__rhs);
        }
    }
    (*buffer.borrow())
        .offset((*i.borrow()) as isize)
        .write((0 as u8));
    return (*i.borrow());
}
pub fn DumpStack_22(format: Ptr<u8>, __args: &[VaArg]) -> i32 {
    let format: Value<Ptr<u8>> = Rc::new(RefCell::new(format));
    let gotSomething: Value<i32> = Rc::new(RefCell::new(((0 == 1) as i32)));
    let fd: Value<i32> = <Value<i32>>::default();
    let pid: Value<i32> = <Value<i32>>::default();
    let status: Value<i32> = Rc::new(RefCell::new(1));
    let rc: Value<i32> = <Value<i32>>::default();
    let args: Value<VaList> = Rc::new(RefCell::new(VaList::default()));
    let cmd: Value<Box<[u8]>> = Rc::new(RefCell::new(
        (0..512).map(|_| <u8>::default()).collect::<Box<[u8]>>(),
    ));
    (*args.borrow_mut()) = VaList::new(__args);
    if ((({
        let _lhs = ({
            let ___str: Ptr<u8> = (cmd.as_pointer() as Ptr<u8>);
            let ___size: usize = ::std::mem::size_of::<[u8; 512]>();
            vsnprintf_3(
                ___str,
                ___size,
                (*format.borrow()).clone(),
                (*args.borrow()).clone(),
            )
        });
        _lhs >= (::std::mem::size_of::<[u8; 512]>() as i32)
    }) as i32)
        != 0)
    {
        return ((0 == 1) as i32);
    };
    (*fd.borrow_mut()) = ({ my_popen_17((cmd.as_pointer() as Ptr<u8>), (pid.as_pointer())) });
    if (((-1_i32 != (*fd.borrow())) as i32) != 0) {
        let mut __do_while = true;
        'loop_: while __do_while
            || ((((((-1_i32 == (*rc.borrow())) as i32) != 0)
                && (((4 == (libcc2rs::cpp2rust_errno().read())) as i32) != 0))
                as i32)
                != 0)
        {
            __do_while = false;
            (*rc.borrow_mut()) = ({ waitpid_23((*pid.borrow()), (status.as_pointer()), 0) });
        }
        (*gotSomething.borrow_mut()) = (!(((0 == 1) as i32) != 0) as i32);
        ({ my_getline_21(-1_i32, Ptr::<u8>::null(), 0) });
        ({ my_pclose_15((*fd.borrow()), (*pid.borrow())) });
    }
    return (*gotSomething.borrow());
}
pub fn StackTrace_24(gdb_command_file: Ptr<u8>) {
    let gdb_command_file: Value<Ptr<u8>> = Rc::new(RefCell::new(gdb_command_file));
    let quoted_progname: Value<Box<[u8]>> = Rc::new(RefCell::new(
        (0..512).map(|_| <u8>::default()).collect::<Box<[u8]>>(),
    ));
    if (((((((*global_progname_0.with(Value::clone).borrow()).is_null()) as i32) != 0)
        || (((({
            let _dst: Ptr<u8> = (quoted_progname.as_pointer() as Ptr<u8>);
            let _dst_size: usize = ::std::mem::size_of::<[u8; 512]>();
            stacktrace_shell_quote_7(
                _dst,
                _dst_size,
                (*global_progname_0.with(Value::clone).borrow()).clone(),
            )
        }) != 0) as i32)
            != 0)) as i32)
        != 0)
    {
        return;
    }
    let cmd: Value<Box<[u8]>> = Rc::new(RefCell::new(
        (0..512).map(|_| <u8>::default()).collect::<Box<[u8]>>(),
    ));
    if ((((((({
        let _dst: Ptr<u8> = (cmd.as_pointer() as Ptr<u8>);
        let _dst_size: usize = ::std::mem::size_of::<[u8; 512]>();
        stacktrace_build_debugger_command_13(
            _dst,
            _dst_size,
            (*global_progname_0.with(Value::clone).borrow()).clone(),
            (*gdb_command_file.borrow()).clone(),
        )
    }) == 0) as i32)
        != 0)
        && (({
            DumpStack_22(
                Ptr::from_string_literal(b"%s"),
                &[(cmd.as_pointer() as Ptr<u8>).into()],
            )
        }) != 0)) as i32)
        != 0)
    {
        return;
    }
    let err_msg: Value<Ptr<u8>> = Rc::new(RefCell::new(Ptr::from_string_literal(
        b"No debugger found\n",
    )));
    let io_err: Value<isize> = Rc::new(RefCell::new(libc::write(
        (*global_output_1.with(Value::clone).borrow()),
        ((*err_msg.borrow()).clone() as Ptr<u8>).to_any(),
        (*err_msg.borrow()).to_string_iterator().count(),
    )));
    if ((((*io_err.borrow()) < 0_isize) as i32) != 0) {
        println!(
            "DEBUG: I/O error writing to global_output - err: ({})",
            (*err_msg.borrow())
        );
    }
}
pub fn StackTraceFromSafeContext_25(gdb_command_file: Ptr<u8>) -> i32 {
    let gdb_command_file: Value<Ptr<u8>> = Rc::new(RefCell::new(gdb_command_file));
    if ((((*gdb_command_file.borrow()).is_null()) as i32) != 0) {
        return 0;
    }
    ({ StackTrace_24((*gdb_command_file.borrow()).reinterpret_cast::<u8>()) });
    return 1;
}
pub fn StackTraceInit_26(in_name: Ptr<u8>, in_handle: i32) {
    let in_name: Value<Ptr<u8>> = Rc::new(RefCell::new(in_name));
    let in_handle: Value<i32> = Rc::new(RefCell::new(in_handle));
    (*global_progname_0.with(Value::clone).borrow_mut()) = (*in_name.borrow()).clone();
    (*global_output_1.with(Value::clone).borrow_mut()) =
        if ((((*in_handle.borrow()) == -1_i32) as i32) != 0) {
            1
        } else {
            (*in_handle.borrow())
        };
}
