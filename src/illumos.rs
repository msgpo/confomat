/*
 * Copyright 2020 Oxide Computer Company
 */

use std::os::raw::{c_char, c_int};
use std::process::exit;
use std::ffi::{CString, CStr};
use std::collections::HashMap;
use anyhow::{Result, bail};

#[derive(Debug, PartialEq)]
pub struct UserAttr {
    pub name: String,
    pub attr: HashMap<String, String>,
}

impl UserAttr {
    pub fn profiles(&self) -> Vec<String> {
        if let Some(p) = self.attr.get("profiles") {
            p.split(',')
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    }
}

#[repr(C)]
struct Kv {
    key: *const c_char,
    value: *const c_char,
}

impl Kv {
    fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.key) }
    }

    fn value(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.value) }
    }
}

#[repr(C)]
struct Kva {
    length: c_int,
    data: *const Kv,
}

impl Kva {
    fn values(&self) -> &[Kv] {
        unsafe { std::slice::from_raw_parts(self.data, self.length as usize) }
    }
}

#[repr(C)]
struct UserAttrRaw {
    name: *mut c_char,
    qualifier: *mut c_char,
    res1: *mut c_char,
    res2: *mut c_char,
    attr: *mut Kva,
}

#[link(name = "secdb")]
extern {
    fn getusernam(buf: *const c_char) -> *mut UserAttrRaw;
    fn free_userattr(userattr: *mut UserAttrRaw);
}

pub fn get_user_attr_by_name(name: &str) -> Result<Option<UserAttr>> {
    let mut out = UserAttr {
        name: name.to_string(),
        attr: HashMap::new(),
    };

    let name = CString::new(name.to_owned())?;
    let ua = unsafe { getusernam(name.as_ptr()) };
    if ua.is_null() {
        return Ok(None);
    }

    for kv in unsafe { (*(*ua).attr).values() } {
        if let (Ok(k), Ok(v)) = (kv.name().to_str(), kv.value().to_str()) {
            out.attr.insert(k.to_string(), v.to_string());
        } else {
            continue;
        }
    }

    unsafe { free_userattr(ua) };

    Ok(Some(out))
}

pub fn nodename() -> String {
    unsafe {
        let mut un: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut un) < 0 {
            eprintln!("uname failure");
            exit(100);
        }
        std::ffi::CStr::from_ptr(un.nodename.as_mut_ptr())
    }.to_str().unwrap().to_string()
}

#[link(name = "c")]
extern {
    fn getzoneid() -> i32;
    fn getzonenamebyid(id: i32, buf: *mut u8, buflen: usize) -> isize;
}

pub fn zoneid() -> i32 {
    unsafe { getzoneid() }
}

pub fn zonename() -> String {
    let buf = unsafe {
        let mut buf: [u8; 64] = std::mem::zeroed(); /* ZONENAME_MAX */

        let sz = getzonenamebyid(getzoneid(), buf.as_mut_ptr(), 64);
        if sz > 64 || sz < 0 {
            eprintln!("getzonenamebyid failure");
            exit(100);
        }

        Vec::from(&buf[0..sz as usize])
    };
    std::ffi::CStr::from_bytes_with_nul(&buf)
        .unwrap().to_str().unwrap().to_string()
}

fn errno() -> i32 {
    unsafe {
        let enp = libc::___errno();
        *enp
    }
}

fn clear_errno() {
    unsafe {
        let enp = libc::___errno();
        *enp = 0;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Passwd {
    pub name: Option<String>,
    pub passwd: Option<String>,
    pub uid: u32,
    pub gid: u32,
    pub age: Option<String>,
    pub comment: Option<String>,
    pub gecos: Option<String>,
    pub dir: Option<String>,
    pub shell: Option<String>,
}

impl Passwd {
    fn from(p: *const libc::passwd) -> Result<Passwd> {
        fn cs(lpsz: *const c_char) -> Result<Option<String>> {
            if lpsz.is_null() {
                Ok(None)
            } else {
                let cstr = unsafe { CStr::from_ptr(lpsz) };
                Ok(Some(cstr.to_str()?.to_string()))
            }
        }

        Ok(Passwd {
            name: cs(unsafe { (*p).pw_name })?,
            passwd: cs(unsafe { (*p).pw_passwd })?,
            uid: unsafe { (*p).pw_uid },
            gid: unsafe { (*p).pw_gid },
            age: cs(unsafe { (*p).pw_age })?,
            comment: cs(unsafe { (*p).pw_comment })?,
            gecos: cs(unsafe { (*p).pw_gecos })?,
            dir: cs(unsafe { (*p).pw_dir })?,
            shell: cs(unsafe { (*p).pw_shell })?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub name: Option<String>,
    pub passwd: Option<String>,
    pub gid: u32,
    pub members: Option<Vec<String>>,
}

impl Group {
    fn from(g: *mut libc::group) -> Result<Group> {
        fn cs(lpsz: *const c_char) -> Result<Option<String>> {
            if lpsz.is_null() {
                Ok(None)
            } else {
                let cstr = unsafe { CStr::from_ptr(lpsz) };
                Ok(Some(cstr.to_str()?.to_string()))
            }
        }

        let mut mems = unsafe { (*g).gr_mem };
        let members: Option<Vec<String>> = if !mems.is_null() {
            let mut members = Vec::new();
            loop {
                if unsafe { *mems }.is_null() {
                    break;
                }

                members.push(cs(unsafe { *mems })?.unwrap());

                mems = unsafe { mems.offset(1) };
             }
            Some(members)
        } else {
            None
        };

        Ok(Group {
            name: cs(unsafe { (*g).gr_name })?,
            passwd: cs(unsafe { (*g).gr_passwd })?,
            gid: unsafe { (*g).gr_gid },
            members,
        })
    }
}

pub fn get_passwd_by_id(uid: u32) -> Result<Option<Passwd>> {
    clear_errno();
    let p = unsafe { libc::getpwuid(uid) };
    let e = errno();
    if p.is_null() {
        if e == 0 {
            Ok(None)
        } else {
            bail!("getpwuid: errno {}", e);
        }
    } else {
        Ok(Some(Passwd::from(p)?))
    }
}

pub fn get_passwd_by_name(name: &str) -> Result<Option<Passwd>> {
    clear_errno();
    let name = CString::new(name.to_owned())?;
    let p = unsafe { libc::getpwnam(name.as_ptr()) };
    let e = errno();
    if p.is_null() {
        if e == 0 {
            Ok(None)
        } else {
            bail!("getpwnam: errno {}", e);
        }
    } else {
        Ok(Some(Passwd::from(p)?))
    }
}

pub fn get_group_by_name(name: &str) -> Result<Option<Group>> {
    clear_errno();
    let name = CString::new(name.to_owned())?;
    let g = unsafe { libc::getgrnam(name.as_ptr()) };
    let e = errno();
    if g.is_null() {
        if e == 0 {
            Ok(None)
        } else {
            bail!("getgrnam: errno {}", e);
        }
    } else {
        Ok(Some(Group::from(g)?))
    }
}

pub fn get_group_by_id(gid: u32) -> Result<Option<Group>> {
    clear_errno();
    let g = unsafe { libc::getgrgid(gid) };
    let e = errno();
    if g.is_null() {
        if e == 0 {
            Ok(None)
        } else {
            bail!("getgrgid: errno {}", e);
        }
    } else {
        Ok(Some(Group::from(g)?))
    }
}
