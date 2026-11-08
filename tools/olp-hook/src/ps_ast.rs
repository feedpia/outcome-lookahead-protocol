//! PowerShell AST fact extractor.
//! Reads PowerShell command text with Windows PowerShell's own parser (`Parser::ParseInput`, **no execution**) and accepts only fact lines.
//! - The script is fixed text (`-EncodedCommand`); the command text goes **only via stdin** (no interpolation). No `-File`, no profile.
//! - The executable is the absolute path `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` (no PATH lookup).
//! - 1.5 s limit, failure, restricted language mode, missing parser, non-Windows or `OLP_HOOK_PS_PARSER=off` → None (the caller falls back to the current list-based decision).
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_millis(1500);

/// /// Output shape (one line = one fact, fields tab-separated, UTF-8): `E <error count>` · `N <UTF-16 length read>` · `C <call operator>\t<s:text|p:name|b:|x:text>…`
/// /// (colon argument `-P:$x` = `p:P` followed by one more `x:`/`b:` field) · `M <0|1 static>\t<type name>\t<method name|?>` · `R <redirect target>` ·
/// /// `A` (assignment to something other than a plain variable, `++`/`--`, `foreach` loop variable — properties, `${c:file}`, `$alias:x`) · `T <type name>` (cast `[type]value` and `value -as [type]`; both can invoke constructors) · `X <node kind>` (outside the allow list = unknown). Tabs/newlines inside values become spaces.
/// /// Allow list: every node outside the allow list `$ok` is reported as `X`, so a node not emitted as a fact never passes silently. The list is pinned literally by a test.
const SCRIPT: &str = r#"$ErrorActionPreference='Stop'
$i=[Console]::OpenStandardInput();$ms=New-Object System.IO.MemoryStream;$i.CopyTo($ms)
$src=[System.Text.Encoding]::UTF8.GetString($ms.ToArray())
$t=$null;$e=$null
$ast=[System.Management.Automation.Language.Parser]::ParseInput($src,[ref]$t,[ref]$e)
function V($s){ if($null -eq $s){''} else {([string]$s) -replace "[`t`r`n]",' '} }
function P($x){ $x -is [System.Management.Automation.Language.VariableExpressionAst] -and -not $x.VariablePath.DriveName }
function W($x){ if($x -is [System.Management.Automation.Language.ScriptBlockExpressionAst]){'b:'} else {'x:'+(V $x.Extent.Text)} }
$ok=@('ArrayExpressionAst','ArrayLiteralAst','AssignmentStatementAst','BinaryExpressionAst','BreakStatementAst','CatchClauseAst','CommandAst','CommandExpressionAst','CommandParameterAst','ConstantExpressionAst','ContinueStatementAst','ConvertExpressionAst','DoUntilStatementAst','DoWhileStatementAst','ExpandableStringExpressionAst','FileRedirectionAst','ForEachStatementAst','ForStatementAst','HashtableAst','IfStatementAst','IndexExpressionAst','InvokeMemberExpressionAst','MemberExpressionAst','MergingRedirectionAst','NamedBlockAst','ParenExpressionAst','PipelineAst','ReturnStatementAst','ScriptBlockAst','ScriptBlockExpressionAst','StatementBlockAst','StringConstantExpressionAst','SubExpressionAst','TryStatementAst','TypeConstraintAst','TypeExpressionAst','UnaryExpressionAst','VariableExpressionAst','WhileStatementAst')
$L=New-Object System.Collections.Generic.List[string]
$L.Add('E '+$e.Count)
$L.Add('N '+$src.Length)
foreach($n in $ast.FindAll({$true},$true)){
 $k=$n.GetType().Name
 if($ok -notcontains $k){ $L.Add('X '+$k); continue }
 if($n -is [System.Management.Automation.Language.CommandAst]){
  $w=foreach($x in $n.CommandElements){
   if($x -is [System.Management.Automation.Language.StringConstantExpressionAst]){'s:'+(V $x.Value)}
   elseif($x -is [System.Management.Automation.Language.CommandParameterAst]){'p:'+(V $x.ParameterName); if($x.Argument){W $x.Argument}}
   else{W $x}}
  $L.Add('C '+$n.InvocationOperator+"`t"+($w -join "`t"))
 } elseif($n -is [System.Management.Automation.Language.InvokeMemberExpressionAst]){
  $m=if($n.Member -is [System.Management.Automation.Language.StringConstantExpressionAst]){V $n.Member.Value}else{'?'}
  $ty=if($n.Static -and $n.Expression -is [System.Management.Automation.Language.TypeExpressionAst]){V $n.Expression.TypeName.FullName}else{''}
  $L.Add('M '+[int]$n.Static+"`t"+$ty+"`t"+$m)
 } elseif($n -is [System.Management.Automation.Language.FileRedirectionAst]){
  $L.Add('R '+(V $n.Location.Extent.Text))
 } elseif($n -is [System.Management.Automation.Language.AssignmentStatementAst] -and -not (P $n.Left)){
  $L.Add('A')
 } elseif($n -is [System.Management.Automation.Language.UnaryExpressionAst] -and "$($n.TokenKind)" -match 'PlusPlus|MinusMinus' -and -not (P $n.Child)){
  $L.Add('A')
 } elseif($n -is [System.Management.Automation.Language.ForEachStatementAst] -and -not (P $n.Variable)){
  $L.Add('A')
 } elseif($n -is [System.Management.Automation.Language.ConvertExpressionAst]){
  $L.Add('T '+(V $n.Type.TypeName.FullName))
 } elseif($n -is [System.Management.Automation.Language.BinaryExpressionAst] -and "$($n.Operator)" -eq 'As'){
  if($n.Right -is [System.Management.Automation.Language.TypeExpressionAst]){ $L.Add('T '+(V $n.Right.TypeName.FullName)) } else { $L.Add('X BinaryExpression(As)') }
 }
}
$b=[System.Text.Encoding]::UTF8.GetBytes(($L -join "`n")+"`n");$o=[Console]::OpenStandardOutput();$o.Write($b,0,$b.Length);$o.Flush()
"#;

/// /// One fact returned by the parser.
#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    ///     /// Command: call operator (`Unknown`, `Ampersand`, `Dot`), words (kind tag, value)
    Command { op: String, elems: Vec<(char, String)> },
    ///     /// Method call: whether static, type name (only when static and a type literal), method name (`?` = not a literal)
    Method { is_static: bool, ty: String, name: String },
    Redirect(String),
    ///     /// Assignment to something other than a variable (e.g. a property)
    Assign,
    ///     /// Cast `[type]value` (if the value is not a type, it may invoke a constructor = unknown)
    Convert(String),
    ///     /// Node outside the allow list (node type name) = unknown
    Unlisted(String),
}

fn exe() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

/// /// UTF-16LE → base64 (`-EncodedCommand` format). No new package.
fn encoded_script() -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = SCRIPT.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for (i, s) in [18, 12, 6, 0].iter().enumerate() {
            out.push(if i <= c.len() { T[(n >> s & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// /// Whether the parser path is attempted (Windows and `OLP_HOOK_PS_PARSER` not `off`). Tests use this value to set their expectations.
pub fn enabled() -> bool {
    cfg!(windows) && !std::env::var("OLP_HOOK_PS_PARSER").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
}

/// /// Command text → facts. None = parser unavailable (the caller decides with the current list).
pub fn parse(cmd: &str) -> Option<Vec<Fact>> {
    if !enabled() {
        return None;
    }
    let mut c = Command::new(exe());
    c.args(["-NoProfile", "-NonInteractive", "-NoLogo", "-EncodedCommand", &encoded_script()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = c.spawn().ok()?;
    let start = Instant::now();
    let mut stdin = child.stdin.take()?;
    let mut stdout = child.stdout.take()?;
    let input = cmd.as_bytes().to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if start.elapsed() < LIMIT => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    read_facts(&String::from_utf8(reader.join().ok()?).ok()?, cmd.encode_utf16().count())
}

/// /// Reads the output shape. None if the first line is not `E 0` (syntax error or unexpected shape), or if the second line `N` differs from the sent length (UTF-16),
/// /// which means the parser read a partial input.
pub fn read_facts(out: &str, want_len: usize) -> Option<Vec<Fact>> {
    let mut lines = out.lines();
    if lines.next()? != "E 0" || lines.next()? != format!("N {want_len}") {
        return None;
    }
    let mut facts = Vec::new();
    for l in lines {
        let (tag, rest) = l.split_at(l.find(' ').unwrap_or(l.len()));
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let f = match tag {
            "C" => {
                let mut cols = rest.split('\t');
                let op = cols.next()?.to_string();
                let elems = cols.map(|e| e.split_once(':').map(|(k, v)| (k.chars().next().unwrap_or('x'), v.to_string()))).collect::<Option<Vec<_>>>()?;
                Fact::Command { op, elems }
            }
            "M" => {
                let mut cols = rest.split('\t');
                Fact::Method { is_static: cols.next()? == "1", ty: cols.next()?.to_string(), name: cols.next()?.to_string() }
            }
            "R" => Fact::Redirect(rest.to_string()),
            "A" => Fact::Assign,
            "T" => Fact::Convert(rest.to_string()),
            "X" => Fact::Unlisted(rest.to_string()),
            _ => return None,
        };
        facts.push(f);
    }
    Some(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_script_round_trips() {
        //         // Decode standard base64 back and check it equals the original UTF-16LE (verifies the hand-written encoder, no new package)
        let enc = encoded_script();
        let t = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = Vec::new();
        for ch in enc.bytes().filter(|&b| b != b'=') {
            let v = t.iter().position(|&x| x == ch).unwrap() as u32;
            bits.push(v);
        }
        let mut bytes = Vec::new();
        for c in bits.chunks(4) {
            let n = c.iter().enumerate().fold(0u32, |n, (i, v)| n | v << (18 - 6 * i));
            for (i, s) in [16, 8, 0].iter().enumerate() {
                if i + 1 < c.len() {
                    bytes.push((n >> s & 255) as u8);
                }
            }
        }
        let want: Vec<u8> = SCRIPT.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(bytes, want);
    }

    #[test]
    fn read_facts_shapes() {
        let f = read_facts("E 0\nN 9\nC Unknown\ts:Get-ChildItem\ts:build\nC Unknown\ts:ForEach-Object\tb:\nM 0\t\tDelete\nM 1\tIO.File\tDelete\nR $null\nA\nT int\nX UsingStatementAst\n", 9).unwrap();
        assert_eq!(f.len(), 8);
        assert_eq!(f[0], Fact::Command { op: "Unknown".into(), elems: vec![('s', "Get-ChildItem".into()), ('s', "build".into())] });
        assert_eq!(f[3], Fact::Method { is_static: true, ty: "IO.File".into(), name: "Delete".into() });
        assert_eq!(f[7], Fact::Unlisted("UsingStatementAst".into()));
        assert!(read_facts("E 1\nN 9\n", 9).is_none(), "구문 오류 = 해석기 결과 안 씀");
        assert!(read_facts("hello\n", 9).is_none());
        assert!(read_facts("E 0\nN 9\nZ what\n", 9).is_none(), "모르는 줄 = 안 씀");
    }

    //     // If the length the parser read differs from the length sent (partial or empty input), the result is not used
    #[test]
    fn f4_partial_or_empty_input_is_not_used() {
        assert!(read_facts("E 0\nN 4\nC Unknown\ts:Get-ChildItem\n", 20).is_none(), "쪽 입력");
        assert!(read_facts("E 0\nN 0\n", 20).is_none(), "빈 입력");
        assert!(read_facts("E 0\nC Unknown\ts:Get-ChildItem\n", 20).is_none(), "길이 줄 없음");
        assert!(read_facts("E 0\nN 20\n", 20).is_some());
    }
}
