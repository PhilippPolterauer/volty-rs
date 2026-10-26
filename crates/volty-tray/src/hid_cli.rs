#[cfg(windows)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
#[cfg(windows)]
use std::rc::Rc;

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
        "inspect" => inspect(options),
        "features" => features(options),
        "run" => run_profile(options),
        "raw" => raw(options),
        _ => Err(error(format!(
            "unknown HID command '{command}'\n{HID_USAGE}"
        ))),
    }
}

#[cfg(windows)]
const HID_USAGE: &str = "Usage: volty hid <inspect|features|run|raw> [options]\n\
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
        "run" => Some(
            "Usage: volty hid run --profile FILE --command NAME --device-index NUMBER [interface selector] [--allow-state-changing]\n\
Loads a TOML command profile, validates it, and executes the named command.\n\
  --profile FILE               TOML profile path\n\
  --command NAME               Command name from the profile\n\
  --device-index NUMBER        HID++ device index (not the USB interface number)\n\
  --allow-state-changing       Authorize a profile command marked state-changing\n\
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
        _ => None,
    }
}

#[cfg(windows)]
#[derive(Default)]
struct Options {
    values: HashMap<String, String>,
    allow_state_changing: bool,
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
            "--allow-state-changing is only valid for the run and raw commands",
        ))
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
            .map(|value| {
                parse_number(value, "--interface").and_then(|value| {
                    i32::try_from(value).map_err(|_| error("invalid interface number"))
                })
            })
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
            .map(|value| format!("0x{value:04x}"))
            .unwrap_or_else(|| "unknown".to_owned()),
        info.usage
            .map(|value| format!("0x{value:04x}"))
            .unwrap_or_else(|| "unknown".to_owned()),
        info.interface_number,
        info.bus_type,
        info.product_string.as_deref().unwrap_or("unknown"),
        info.path.to_string_lossy(),
    )
}

#[cfg(windows)]
fn inspect(options: Options) -> Result<(), CliError> {
    reject_state_flag(&options)?;
    check_options(
        &options,
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
    let selector = selector(&options)?;
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
        1 => Ok(matches.into_iter().next().unwrap()),
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
#[derive(Default)]
struct FrameTrace {
    requests: Vec<Vec<u8>>,
    responses: Vec<Vec<u8>>,
}

#[cfg(windows)]
struct DiagnosticTransport<T> {
    inner: T,
    trace: Rc<RefCell<FrameTrace>>,
}

#[cfg(windows)]
impl<T: volty_hid::HidppTransport> volty_hid::HidppTransport for DiagnosticTransport<T> {
    fn transact(
        &mut self,
        request: &volty_hid::HidppFrame,
        timeout: std::time::Duration,
    ) -> Result<volty_hid::HidppFrame, volty_hid::HidppError> {
        if let Ok(bytes) = request.encode() {
            self.trace.borrow_mut().requests.push(bytes);
        }
        let response = volty_hid::HidppTransport::transact(&mut self.inner, request, timeout)?;
        if let Ok(bytes) = response.encode() {
            self.trace.borrow_mut().responses.push(bytes);
        }
        Ok(response)
    }
}

#[cfg(windows)]
fn print_trace(trace: &FrameTrace) {
    for (index, frame) in trace.requests.iter().enumerate() {
        println!("TX[{index}]: {frame:02x?}");
    }
    for (index, frame) in trace.responses.iter().enumerate() {
        println!("RX[{index}]: {frame:02x?}");
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
fn features(options: Options) -> Result<(), CliError> {
    reject_state_flag(&options)?;
    check_options(
        &options,
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
    let device_index = device_index(&options)?;
    let selector = selector(&options)?;
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
fn run_profile(options: Options) -> Result<(), CliError> {
    check_options(
        &options,
        &[
            "--profile",
            "--command",
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
    let profile_path = required(&options, "--profile")?;
    let command_name = required(&options, "--command")?;
    let device_index = device_index(&options)?;
    let contents = std::fs::read_to_string(profile_path)
        .map_err(|cause| error(format!("cannot read profile '{}': {cause}", profile_path)))?;
    let profile = volty_hid::CommandProfile::from_toml(&contents)
        .map_err(|cause| error(format!("invalid TOML profile: {cause}")))?;
    if options.allow_state_changing {
        println!("WARNING: state-changing commands in this profile are authorized.");
    }

    let selector = selector(&options)?;
    let session = volty_hid::HidppSession::open(&selector).map_err(|error| error.to_string())?;
    let trace = Rc::new(RefCell::new(FrameTrace::default()));
    let transport = DiagnosticTransport {
        inner: session,
        trace: Rc::clone(&trace),
    };
    let mut executor = volty_hid::CommandExecutor::new(transport);
    let result = executor
        .execute(
            &profile,
            command_name,
            device_index,
            options.allow_state_changing,
        )
        .map_err(|cause| error(cause.to_string()));

    print_trace(&trace.borrow());
    let result = result?;
    println!("decoded: {:?}", result.decoded);
    Ok(())
}

#[cfg(windows)]
fn raw(options: Options) -> Result<(), CliError> {
    check_options(
        &options,
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

    let device_index = device_index(&options)?;
    let feature_index = parse_byte(required(&options, "--feature-index")?, "--feature-index")?;
    let function_index = parse_byte(required(&options, "--function-index")?, "--function-index")?;
    if function_index > 0x0f {
        return Err(error("--function-index must be in 0..=15"));
    }
    let report_type = required(&options, "--report-type")?;
    let params = parse_hex_bytes(required(&options, "--params")?)?;
    let frame = match report_type {
        "short" if params.len() == 3 => {
            let params: [u8; 3] = params.try_into().unwrap();
            volty_hid::HidppFrame::short(
                device_index,
                feature_index,
                function_index,
                volty_hid::HIDPP_SOFTWARE_ID,
                params,
            )
        }
        "long" if params.len() == 16 => {
            let params: [u8; 16] = params.try_into().unwrap();
            volty_hid::HidppFrame::long(
                device_index,
                feature_index,
                function_index,
                volty_hid::HIDPP_SOFTWARE_ID,
                params,
            )
        }
        "short" => {
            return Err(error(format!(
                "short reports require exactly 3 parameter bytes, got {}",
                params.len()
            )))
        }
        "long" => {
            return Err(error(format!(
                "long reports require exactly 16 parameter bytes, got {}",
                params.len()
            )))
        }
        _ => return Err(error("--report-type must be 'short' or 'long'")),
    };
    let request = frame.encode().map_err(|error| error.to_string())?;

    let selector = selector(&options)?;
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
