//! Formatting helpers for detailed callee output.

use super::callees_detail::CallSite;

pub fn format_call_site(site: &CallSite) -> String {
    let mut out = format!("L{}", site.line);

    if let Some(ref ret) = site.return_var {
        out.push_str(&format!(" {ret}"));
        out.push_str(&format!(" = {}", site.callee));
    } else if site.is_return {
        out.push_str(&format!(" ->ret {}", site.callee));
    } else {
        out.push_str(&format!(" {}", site.callee));
    }

    if !site.args.is_empty() {
        let args: Vec<String> = site.args.iter().map(|a| compact_arg(a)).collect();
        out.push_str(&format!("({})", args.join(", ")));
    } else {
        out.push_str("()");
    }

    out
}

fn compact_arg(arg: &str) -> String {
    if arg.len() <= 20 {
        arg.to_string()
    } else {
        let mut end = 17;
        while end > 0 && !arg.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &arg[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(
        line: u32,
        callee: &str,
        args: Vec<String>,
        ret: Option<String>,
        is_ret: bool,
    ) -> CallSite {
        CallSite {
            line,
            callee: callee.to_string(),
            call_text: String::new(),
            args,
            return_var: ret,
            is_return: is_ret,
        }
    }

    #[test]
    fn formats_assignment() {
        let s = format_call_site(&site(
            42,
            "foo",
            vec!["x".into(), "y".into()],
            Some("result".into()),
            false,
        ));
        assert_eq!(s, "L42 result = foo(x, y)");
    }

    #[test]
    fn formats_bare_call() {
        let s = format_call_site(&site(10, "bar", vec![], None, false));
        assert_eq!(s, "L10 bar()");
    }

    #[test]
    fn formats_return_call() {
        let s = format_call_site(&site(5, "baz", vec!["ctx".into()], None, true));
        assert_eq!(s, "L5 ->ret baz(ctx)");
    }

    #[test]
    fn compacts_long_arg() {
        let long = "a_very_long_argument_name_that_exceeds_limit";
        let s = format_call_site(&site(1, "f", vec![long.into()], None, false));
        assert!(s.contains("a_very_long_argum..."));
    }
}
