//! The record of a user's change (`dyncfg_log_user_action()`, `src/daemon/dyncfg/dyncfg-intercept.c`): who changed
//! which configuration, with the caller's identity as fields.

use netdata_agent_log::{Field, Priority, Source, Value, msgid, nd_log, push};
use netdata_agent_nrpc::access::{UserAuth, role};

use crate::intercept::Call;
use crate::model::{Cmds, Type};

/// A NOTICE for every command but `userconfig`, `get` and `schema`: `DYNCFG USER ACTION '<cmd>' [<name> ]<on> '<id>'
/// by user '<user>', IP '<forwarded for, else the client's>'`, under C's fields (`localhost` is the node).
pub(crate) fn user_action(localhost: &str, kind: Type, call: &Call) {
    if call.cmd == Cmds::USERCONFIG || call.cmd == Cmds::GET || call.cmd == Cmds::SCHEMA {
        return;
    }
    let on = match kind {
        Type::Template => "on template",
        Type::Job => "on job",
        Type::Single => "on",
    };
    // C's `user_auth_from_source()` fails only without a source, which a call always has
    let auth = UserAuth::from_source(&call.source);
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let access = auth.access;
    let _frame = push(vec![
        (Field::Module, Value::txt("DYNCFG")),
        (Field::NidlNode, Value::Str(localhost.to_string())),
        (Field::Request, Value::Txt(text(&call.function))),
        (Field::TransactionId, Value::Uuid(call.transaction)),
        (Field::MessageId, Value::Uuid(msgid::DYNCFG_USER_ACTION)),
        (Field::AccountId, Value::Uuid(auth.account)),
        (Field::SrcIp, Value::Txt(text(&auth.client_ip))),
        (Field::SrcForwardedFor, Value::Txt(text(&auth.forwarded_for))),
        (Field::UserName, Value::Txt(text(&auth.client_name))),
        (Field::UserRole, Value::txt(role::name(auth.role))),
        (
            Field::UserAccess,
            Value::lazy(move |out| {
                out.extend_from_slice(format!("0x{access:x}").as_bytes());
                true
            }),
        ),
    ]);
    let ip = if auth.forwarded_for.is_empty() { &auth.client_ip } else { &auth.forwarded_for };
    let (name, space) = match &call.add_name {
        Some(name) => (text(name), " "),
        None => (String::new(), ""),
    };
    nd_log!(
        Source::Daemon,
        Priority::Notice,
        "DYNCFG USER ACTION '{}' {name}{space}{on} '{}' by user '{}', IP '{}'",
        call.cmd.name_one().unwrap_or("none"),
        text(&call.id),
        text(&auth.client_name),
        text(ip)
    );
}
