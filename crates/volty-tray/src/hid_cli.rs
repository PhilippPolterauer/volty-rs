#[cfg(windows)]
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
#[cfg(windows)]
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    time::{Duration, Instant},
};
#[cfg(windows)]
use volty_hid::HidReportIo;

#[derive(Debug)]
pub struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for CliError {}

fn error(message: impl Into<String>) -> CliError {
    CliError(message.into())
}

#[cfg(not(windows))]
pub fn run(_args: &[String]) -> Result<(), CliError> {
    Err(error(
        "HID diagnostics are supported only on Windows builds of Volty.",
    ))
}

#[cfg(windows)]
pub fn run(args: &[String]) -> Result<(), CliError> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(error(HID_USAGE));
    };
    if matches!(command, "-h" | "--help" | "help") {
        println!("{HID_USAGE}");
        return Ok(());
    }
    if command == "trace" {
        return trace(&args[1..]);
    }

    let options = parse_options(&args[1..])?;
    if options.help {
        println!(
            "{}",
            command_usage(command).ok_or_else(|| {
                error(format!("unknown HID command '{command}'\n{HID_USAGE}"))
            })?
        );
        return Ok(());
    }

    match command {
        "inspect" => inspect(&options),
        "features" => features(&options),
        "raw" => raw(&options),
        "sniff" => sniff(&options),
        _ => Err(error(format!(
            "unknown HID command '{command}'\n{HID_USAGE}"
        ))),
    }
}

#[cfg(windows)]
const HID_USAGE: &str = "Usage: volty hid <inspect|features|raw|sniff|trace> [options]\n\
Run `volty hid <command> --help` for command options.";

#[cfg(windows)]
fn command_usage(command: &str) -> Option<&'static str> {
    match command {
        "inspect" => Some(
            "Usage: volty hid inspect [interface selector]\n\
Lists matching HID interfaces. Paths are shown because this is an explicit diagnostic.\n\
  --vid, --vendor-id HEX       Vendor ID\n\
  --pid, --product-id HEX      Product ID\n\
  --usage-page HEX             HID usage page\n\
  --usage HEX                  HID usage\n\
  --interface NUMBER           HID interface number\n\
  --path PATH                  Exact hidapi path\n\
Report lengths are not exposed by the current hidapi API.",
        ),
        "features" => Some(
            "Usage: volty hid features --device-index NUMBER [interface selector]\n\
Discovers logical feature IDs and their runtime indexes for a paired HID++ device.\n\
  --device-index NUMBER        HID++ device index (not the USB interface number)\n\
  --vid, --vendor-id HEX       Vendor ID\n\
  --pid, --product-id HEX      Product ID\n\
  --usage-page HEX             HID usage page\n\
  --usage HEX                  HID usage\n\
  --interface NUMBER           HID interface number\n\
  --path PATH                  Exact hidapi path",
        ),
        "raw" => Some(
            "Usage: volty hid raw --device-index N --feature-index N --function-index N --report-type short|long --params HEX --allow-state-changing [interface selector]\n\
Sends an arbitrary HID++ request. Raw operations are not assumed to be read-only.\n\
  --device-index NUMBER        HID++ device index (not the USB interface number)\n\
  --feature-index NUMBER       Runtime feature index\n\
  --function-index NUMBER      Function index (0..=15)\n\
  --report-type short|long     Report format; parameter payload must be exactly 3/16 bytes\n\
  --params HEX                 Hex bytes, e.g. 00,00,00 (3 short or 16 long bytes)\n\
  --allow-state-changing       Required acknowledgment for arbitrary raw operations\n\
  --vid, --vendor-id HEX       Vendor ID\n\
  --pid, --product-id HEX      Product ID\n\
  --usage-page HEX             HID usage page\n\
  --usage HEX                  HID usage\n\
  --interface NUMBER           HID interface number\n\
  --path PATH                  Exact hidapi path",
        ),
        "sniff" => Some(
            "Usage: volty hid sniff --output PATH --include-input [--duration-seconds NUMBER] [interface selector]\n\
Records raw input reports from exactly one selected HID interface. Captures are local and can contain button or keyboard input.\n\
  --output PATH                New JSONL trace file; existing files are not overwritten\n\
  --include-input              Acknowledge that raw reports can contain user input\n\
  --duration-seconds NUMBER    Capture duration, 1..=120 (default 10)\n\
  --vid, --vendor-id HEX       Vendor ID\n\
  --pid, --product-id HEX      Product ID\n\
  --usage-page HEX             HID usage page\n\
  --usage HEX                  HID usage\n\
  --interface NUMBER           HID interface number\n\
  --path PATH                  Exact hidapi path",
        ),
        "trace" => Some(
            "Usage: volty hid trace <trace-file>\n\
Validates a raw HID trace and prints report counts by direction, ID, and length.",
        ),
        _ => None,
    }
}

#[cfg(windows)]
#[derive(Default)]
struct Options {
    values: HashMap<String, String>,
    allow_state_changing: bool,
    include_input: bool,
    help: bool,
}

#[cfg(windows)]
fn parse_options(args: &[String]) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let option = &args[index];
        match option.as_str() {
            "-h" | "--help" => options.help = true,
            "--allow-state-changing" => {
                if options.allow_state_changing {
                    return Err(error("duplicate option --allow-state-changing"));
                }
                options.allow_state_changing = true;
            }
            "--include-input" => {
                if options.include_input {
                    return Err(error("duplicate option --include-input"));
                }
                options.include_input = true;
            }
            _ if option.starts_with("--") => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| error(format!("missing value for {option}")))?;
                if value.starts_with("--") {
                    return Err(error(format!("missing value for {option}")));
                }
                if options
                    .values
                    .insert(option.clone(), value.clone())
                    .is_some()
                {
                    return Err(error(format!("duplicate option {option}")));
                }
                index += 1;
            }
            _ => return Err(error(format!("unexpected argument '{option}'"))),
        }
        index += 1;
    }
    Ok(options)
}

#[cfg(windows)]
fn check_options(options: &Options, allowed: &[&str]) -> Result<(), CliError> {
    for option in options.values.keys() {
        if !allowed.contains(&option.as_str()) {
            return Err(error(format!("unknown option {option}")));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn reject_state_flag(options: &Options) -> Result<(), CliError> {
    if options.allow_state_changing {
        Err(error(
            "--allow-state-changing is only valid for the raw command",
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn reject_input_flag(options: &Options) -> Result<(), CliError> {
    if options.include_input {
        Err(error("--include-input is only valid for the sniff command"))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn option<'a>(options: &'a Options, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| options.values.get(*name).map(String::as_str))
}

#[cfg(windows)]
fn parse_number(value: &str, option_name: &str) -> Result<u16, CliError> {
    let (digits, radix) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|digits| (digits, 16))
        .unwrap_or((value, 10));
    u16::from_str_radix(digits, radix)
        .map_err(|_| error(format!("invalid value for {option_name}: '{value}'")))
}

#[cfg(windows)]
fn parse_hex_number(value: &str, option_name: &str) -> Result<u16, CliError> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u16::from_str_radix(digits, 16).map_err(|_| {
        error(format!(
            "invalid hexadecimal value for {option_name}: '{value}'"
        ))
    })
}

#[cfg(windows)]
fn parse_byte(value: &str, option_name: &str) -> Result<u8, CliError> {
    let value = parse_number(value, option_name)?;
    u8::try_from(value).map_err(|_| error(format!("{option_name} must be in 0..=255")))
}

#[cfg(windows)]
fn selector(options: &Options) -> Result<volty_hid::HidInterfaceSelector, CliError> {
    for aliases in [
        &["--vid", "--vendor-id"][..],
        &["--pid", "--product-id"][..],
        &["--interface", "--interface-number"][..],
    ] {
        if aliases
            .iter()
            .filter(|name| options.values.contains_key(**name))
            .count()
            > 1
        {
            return Err(error(format!(
                "use only one spelling of selector option {}",
                aliases[0]
            )));
        }
    }
    Ok(volty_hid::HidInterfaceSelector {
        vendor_id: option(options, &["--vid", "--vendor-id"])
            .map(|value| parse_hex_number(value, "--vid"))
            .transpose()?,
        product_id: option(options, &["--pid", "--product-id"])
            .map(|value| parse_hex_number(value, "--pid"))
            .transpose()?,
        usage_page: option(options, &["--usage-page"])
            .map(|value| parse_hex_number(value, "--usage-page"))
            .transpose()?,
        usage: option(options, &["--usage"])
            .map(|value| parse_hex_number(value, "--usage"))
            .transpose()?,
        interface_number: option(options, &["--interface", "--interface-number"])
            .map(|value| parse_number(value, "--interface").map(i32::from))
            .transpose()?,
        path: option(options, &["--path"]).map(str::to_owned),
    })
}

#[cfg(windows)]
fn matches_selector(
    info: &volty_hid::HidInterfaceInfo,
    selector: &volty_hid::HidInterfaceSelector,
) -> bool {
    selector.matches(info)
}

#[cfg(windows)]
fn enumerate_interfaces() -> Result<Vec<volty_hid::HidInterfaceInfo>, CliError> {
    volty_hid::enumerate_hid_interfaces().map_err(|error| error.to_string().into())
}

#[cfg(windows)]
impl From<String> for CliError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(windows)]
fn format_interface(info: &volty_hid::HidInterfaceInfo) -> String {
    format!(
        "VID:PID={:04x}:{:04x} usage={}/{} interface={} bus={} product={} path={}",
        info.vendor_id,
        info.product_id,
        info.usage_page
            .map_or_else(|| "unknown".to_owned(), |value| format!("0x{value:04x}")),
        info.usage
            .map_or_else(|| "unknown".to_owned(), |value| format!("0x{value:04x}")),
        info.interface_number,
        info.bus_type,
        info.product_string.as_deref().unwrap_or("unknown"),
        info.path.to_string_lossy(),
    )
}

#[cfg(windows)]
fn inspect(options: &Options) -> Result<(), CliError> {
    reject_state_flag(options)?;
    reject_input_flag(options)?;
    check_options(
        options,
        &[
            "--vid",
            "--vendor-id",
            "--pid",
            "--product-id",
            "--usage-page",
            "--usage",
            "--interface",
            "--interface-number",
            "--path",
        ],
    )?;
    let selector = selector(options)?;
    let interfaces = enumerate_interfaces()?;
    let mut count = 0;
    for info in interfaces
        .iter()
        .filter(|info| matches_selector(info, &selector))
    {
        println!("{}", format_interface(info));
        count += 1;
    }
    if count == 0 {
        println!("No HID interfaces matched the selector.");
    }
    println!("Report lengths are unavailable from the current hidapi API.");
    Ok(())
}

#[cfg(windows)]
fn select_interface(
    selector: &volty_hid::HidInterfaceSelector,
) -> Result<volty_hid::HidInterfaceInfo, CliError> {
    let matches = enumerate_interfaces()?
        .into_iter()
        .filter(|info| matches_selector(info, selector))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Err(error(
            "no HID interface matched; run `volty hid inspect` and provide an interface selector",
        )),
        1 => {
            matches.into_iter().next().ok_or_else(|| {
                error("no HID interface matched; run `volty hid inspect` and provide an interface selector")
            })
        }
        _ => {
            let choices = matches
                .iter()
                .map(format_interface)
                .collect::<Vec<_>>()
                .join("\n  ");
            Err(error(format!(
                "interface selector matched {} interfaces; narrow it with --vid, --pid, --usage-page, --usage, --interface, or --path:\n  {choices}",
                matches.len()
            )))
        }
    }
}

#[cfg(windows)]
fn required<'a>(options: &'a Options, name: &str) -> Result<&'a str, CliError> {
    option(options, &[name]).ok_or_else(|| error(format!("missing required option {name}")))
}

#[cfg(windows)]
fn device_index(options: &Options) -> Result<u8, CliError> {
    parse_byte(required(options, "--device-index")?, "--device-index")
}

#[cfg(windows)]
fn features(options: &Options) -> Result<(), CliError> {
    reject_state_flag(options)?;
    reject_input_flag(options)?;
    check_options(
        options,
        &[
            "--device-index",
            "--vid",
            "--vendor-id",
            "--pid",
            "--product-id",
            "--usage-page",
            "--usage",
            "--interface",
            "--interface-number",
            "--path",
        ],
    )?;
    let device_index = device_index(options)?;
    let selector = selector(options)?;
    let interface = select_interface(&selector)?;
    let mut session =
        volty_hid::HidppSession::open(&selector).map_err(|error| error.to_string())?;
    let features = session
        .enumerate_features(device_index, std::time::Duration::from_secs(2))
        .map_err(|error| error.to_string())?;

    println!(
        "Interface {}:{} device-index=0x{device_index:02x} (HID++ paired-device index)",
        interface.vendor_id, interface.product_id
    );
    if features.is_empty() {
        println!("No features were discovered.");
    }
    for (logical_id, feature) in features {
        println!(
            "logical-id=0x{:04x} runtime-index=0x{:02x} type=0x{:02x} version={}",
            logical_id, feature.feature_index, feature.feature_type, feature.version
        );
    }
    Ok(())
}

#[cfg(windows)]
fn sniff(options: &Options) -> Result<(), CliError> {
    reject_state_flag(options)?;
    if !options.include_input {
        return Err(error(
            "raw HID reports can contain button or keyboard input; pass --include-input to acknowledge recording it",
        ));
    }
    check_options(
        options,
        &[
            "--output",
            "--duration-seconds",
            "--vid",
            "--vendor-id",
            "--pid",
            "--product-id",
            "--usage-page",
            "--usage",
            "--interface",
            "--interface-number",
            "--path",
        ],
    )?;

    let output = std::path::PathBuf::from(required(options, "--output")?);
    let duration_seconds = option(options, &["--duration-seconds"])
        .map(|value| parse_number(value, "--duration-seconds"))
        .transpose()?
        .unwrap_or(10);
    if !(1..=120).contains(&duration_seconds) {
        return Err(error("--duration-seconds must be in 1..=120"));
    }

    let selector = selector(options)?;
    let interface = select_interface(&selector)?;
    let session = volty_hid::HidReportSession::open_for_interface(&interface)
        .map_err(|error| error.to_string())?;
    let (seconds, nanos) = crate::capture::unix_now().map_err(|error| error.to_string())?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|io_error| {
            error(format!(
                "could not create trace file {} without overwriting an existing file: {io_error}",
                output.display()
            ))
        })?;
    let header = volty_capture::HidTraceHeader {
        schema: volty_capture::HID_TRACE_SCHEMA.to_owned(),
        trace_id: format!("{seconds}-{nanos:09}"),
        created_at: crate::capture::unix_to_rfc3339(seconds),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        interface: volty_capture::HidTraceInterface {
            bus: interface.bus_type.clone(),
            vendor_id: interface.vendor_id,
            product_id: interface.product_id,
            release_number: interface.release_number,
            usage_page: interface.usage_page,
            usage: interface.usage,
            interface_number: interface.interface_number,
        },
    };
    let mut trace =
        volty_capture::HidTraceWriter::new(file, header).map_err(|error| error.to_string())?;
    let mut recorder = volty_hid::RecordingHidIo::new(session);
    let capture_result = capture_input_reports(
        &mut recorder,
        &mut trace,
        Duration::from_secs(u64::from(duration_seconds)),
    );
    let finish_result = trace
        .finish(capture_result.is_ok())
        .map_err(|error| error.to_string());

    match (capture_result, finish_result) {
        (Ok((reports, bytes)), Ok(())) => {
            println!(
                "Captured {reports} input reports ({bytes} bytes) from {:04x}:{:04x} interface {} to {}.",
                interface.vendor_id,
                interface.product_id,
                interface.interface_number,
                output.display()
            );
            println!("The trace contains raw input and is local only; review it before sharing.");
            Ok(())
        }
        (Err(capture_error), Ok(())) => Err(capture_error),
        (Ok(_), Err(finish_error)) => Err(error(finish_error)),
        (Err(capture_error), Err(finish_error)) => Err(error(format!(
            "{capture_error}; could not finalize the partial trace: {finish_error}"
        ))),
    }
}

#[cfg(windows)]
fn capture_input_reports<W: std::io::Write>(
    recorder: &mut volty_hid::RecordingHidIo<volty_hid::HidReportSession>,
    trace: &mut volty_capture::HidTraceWriter<W>,
    duration: Duration,
) -> Result<(u64, u64), CliError> {
    let started = Instant::now();
    let mut buffer = [0u8; volty_capture::MAX_HID_TRACE_REPORT_BYTES];
    let mut report_count = 0u64;
    let mut byte_count = 0u64;

    while started.elapsed() < duration {
        let timeout = duration
            .saturating_sub(started.elapsed())
            .min(Duration::from_millis(100));
        recorder
            .read_input(&mut buffer, timeout)
            .map_err(|io_error| error(io_error.to_string()))?;
        for event in recorder.take_events() {
            let direction = match event.direction {
                volty_hid::HidReportDirection::Input => volty_capture::HidTraceDirection::Input,
                volty_hid::HidReportDirection::Output => volty_capture::HidTraceDirection::Output,
                volty_hid::HidReportDirection::FeatureRead => {
                    volty_capture::HidTraceDirection::FeatureRead
                }
                volty_hid::HidReportDirection::FeatureWrite => {
                    volty_capture::HidTraceDirection::FeatureWrite
                }
            };
            let report = volty_capture::hid_trace_report(event.t_ms, direction, &event.bytes)
                .map_err(|trace_error| error(trace_error.to_string()))?;
            trace
                .write_report(&report)
                .map_err(|trace_error| error(trace_error.to_string()))?;
            report_count += 1;
            byte_count = byte_count
                .checked_add(event.bytes.len() as u64)
                .ok_or_else(|| error("captured HID byte count overflowed"))?;
        }
    }

    Ok((report_count, byte_count))
}

#[cfg(windows)]
fn trace(args: &[String]) -> Result<(), CliError> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        println!("{}", command_usage("trace").unwrap_or(HID_USAGE));
        return Ok(());
    }
    let [path] = args else {
        return Err(error(
            command_usage("trace").unwrap_or(HID_USAGE).to_owned(),
        ));
    };
    let trace = volty_capture::HidTraceFile::read(path).map_err(|error| error.to_string())?;
    let report_timing = trace.reports.last().map_or_else(
        || "no reports".to_owned(),
        |report| format!("last report at {} ms", report.t_ms),
    );
    println!(
        "HID trace {:04x}:{:04x} interface {}: {} reports, {report_timing}, {} bytes, {}.",
        trace.header.interface.vendor_id,
        trace.header.interface.product_id,
        trace.header.interface.interface_number,
        trace.footer.reports,
        trace.footer.bytes,
        if trace.footer.complete {
            "complete"
        } else {
            "incomplete"
        }
    );

    let mut counts = BTreeMap::new();
    for report in &trace.reports {
        *counts
            .entry((report.direction, report.report_id, report.data.len() / 2))
            .or_insert(0u64) += 1;
    }
    for ((direction, report_id, length), count) in counts {
        println!("{direction:?} report_id=0x{report_id:02x} length={length} count={count}");
    }
    println!("Raw report bytes remain in the trace file; treat them as user input.");
    Ok(())
}

#[cfg(windows)]
fn raw(options: &Options) -> Result<(), CliError> {
    reject_input_flag(options)?;
    check_options(
        options,
        &[
            "--device-index",
            "--feature-index",
            "--function-index",
            "--report-type",
            "--params",
            "--vid",
            "--vendor-id",
            "--pid",
            "--product-id",
            "--usage-page",
            "--usage",
            "--interface",
            "--interface-number",
            "--path",
        ],
    )?;
    if !options.allow_state_changing {
        return Err(error(
            "raw commands are arbitrary and may change device state; pass --allow-state-changing to acknowledge this risk",
        ));
    }

    let device_index = device_index(options)?;
    let feature_index = parse_byte(required(options, "--feature-index")?, "--feature-index")?;
    let function_index = parse_byte(required(options, "--function-index")?, "--function-index")?;
    if function_index > 0x0f {
        return Err(error("--function-index must be in 0..=15"));
    }
    let report_type = required(options, "--report-type")?;
    let params = parse_hex_bytes(required(options, "--params")?)?;
    let frame = match report_type {
        "short" => {
            let params: [u8; 3] = params.try_into().map_err(|params: Vec<u8>| {
                error(format!(
                    "short reports require exactly 3 parameter bytes, got {}",
                    params.len()
                ))
            })?;
            volty_hid::HidppFrame::short(
                device_index,
                feature_index,
                function_index,
                volty_hid::HIDPP_SOFTWARE_ID,
                params,
            )
        }
        "long" => {
            let params: [u8; 16] = params.try_into().map_err(|params: Vec<u8>| {
                error(format!(
                    "long reports require exactly 16 parameter bytes, got {}",
                    params.len()
                ))
            })?;
            volty_hid::HidppFrame::long(
                device_index,
                feature_index,
                function_index,
                volty_hid::HIDPP_SOFTWARE_ID,
                params,
            )
        }
        _ => return Err(error("--report-type must be 'short' or 'long'")),
    };
    let request = frame.encode().map_err(|error| error.to_string())?;

    let selector = selector(options)?;
    select_interface(&selector)?;
    let mut session =
        volty_hid::HidppSession::open(&selector).map_err(|error| error.to_string())?;
    println!("WARNING: raw HID++ operations are not assumed to be read-only.");
    println!("TX: {request:02x?}");
    let response = volty_hid::HidppTransport::transact(
        &mut session,
        &frame,
        std::time::Duration::from_secs(2),
    )
    .map_err(|error| error.to_string())?;
    println!(
        "RX: {:02x?}",
        response.encode().map_err(|error| error.to_string())?
    );
    Ok(())
}

#[cfg(windows)]
fn parse_hex_bytes(value: &str) -> Result<Vec<u8>, CliError> {
    let compact = value
        .trim()
        .strip_prefix("0x")
        .or_else(|| value.trim().strip_prefix("0X"))
        .unwrap_or(value.trim())
        .chars()
        .filter(|character| !matches!(character, ',' | ':' | '-' | ' ' | '\t'))
        .collect::<String>();
    if compact.len() % 2 != 0 || !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(error(
            "--params must be an even number of hexadecimal digits; separators are optional",
        ));
    }
    (0..compact.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&compact[index..index + 2], 16)
                .map_err(|_| error("invalid hexadecimal byte in --params"))
        })
        .collect()
}
