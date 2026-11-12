// PCTunerFan - the hardware side of PC Tuner's Fan Tuner tab.
//
// Reads temperatures and fans through LibreHardwareMonitorLib (which talks to the PawnIO driver)
// and drives fan headers on request. PC Tuner starts it hidden and talks over stdin/stdout:
//   helper -> app : one JSON status line per second
//   app -> helper : ping | set <id> <percent> | auto <id> | autoall | pump <id> 0|1 | quit
//
// Safety rules live here, so they hold even if the PC Tuner window freezes or crashes:
//   - fans never below 20 %, pumps never below 50 %
//   - any CPU or GPU temperature >= 90 C: every controlled fan to 100 % until both are below 80 C
//   - if temperatures cannot be read for 5 s while fans are controlled: fans go back to the BIOS
//   - fans go back to the BIOS (SetDefault) on quit, when PC Tuner closes or crashes (stdin closes
//     or its process disappears), when PC Tuner sends nothing for 10 s, and on any error or shutdown.
//
// Other modes: --dump [file] writes a sensor report; --selftest checks the safety logic (no hardware).

using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using LibreHardwareMonitor.Hardware;

namespace PCTunerFan
{
    /// <summary>Pure safety rules (unit-tested by --selftest).</summary>
    public static class Safety
    {
        public const float FanFloor = 20f;
        public const float PumpFloor = 50f;
        public const float HotOn = 90f;
        public const float HotOff = 80f;
        public const double WatchdogSeconds = 10.0;
        public const double BlindSeconds = 5.0;

        /// <summary>Requested duty -> duty that is actually allowed.</summary>
        public static float Clamp(float percent, bool pump)
        {
            if (float.IsNaN(percent)) percent = 100f;
            float floor = pump ? PumpFloor : FanFloor;
            return Math.Max(floor, Math.Min(100f, percent));
        }

        /// <summary>Emergency latch: on at >= 90 C, off only when every known temperature is below 80 C.</summary>
        public static bool Emergency(bool latched, float? cpu, float? gpu)
        {
            bool hot = (cpu ?? 0) >= HotOn || (gpu ?? 0) >= HotOn;
            if (hot) return true;
            if (!latched) return false;
            bool anyKnown = cpu.HasValue || gpu.HasValue;
            bool cool = anyKnown && (cpu ?? 0) < HotOff && (gpu ?? 0) < HotOff;
            return !cool;
        }

        public const float HysteresisC = 3f;

        /// <summary>Duty for a temperature on a curve of (temperature, duty) points, linear in between.</summary>
        public static float CurveDuty(IList<(float t, float d)> points, float temp)
        {
            if (points == null || points.Count == 0) return 100f;
            if (temp <= points[0].t) return points[0].d;
            for (int i = 1; i < points.Count; i++)
            {
                if (temp <= points[i].t)
                {
                    var (t0, d0) = points[i - 1];
                    var (t1, d1) = points[i];
                    return t1 <= t0 ? d1 : d0 + (d1 - d0) * (temp - t0) / (t1 - t0);
                }
            }
            return points[points.Count - 1].d;
        }

        /// <summary>Temperature a curve follows: rises at once, falls only after a 3 C drop (stops fans hunting).</summary>
        public static float Hysteresis(float? last, float temp)
        {
            if (!last.HasValue || temp >= last.Value || last.Value - temp >= HysteresisC) return temp;
            return last.Value;
        }

        public static bool IsPumpName(string name) =>
            name != null && (name.IndexOf("pump", StringComparison.OrdinalIgnoreCase) >= 0 || name.IndexOf("aio", StringComparison.OrdinalIgnoreCase) >= 0);
    }

    class UpdateVisitor : IVisitor
    {
        public void VisitComputer(IComputer computer) => computer.Traverse(this);
        public void VisitHardware(IHardware hardware)
        {
            hardware.Update();
            foreach (var sub in hardware.SubHardware) sub.Accept(this);
        }
        public void VisitSensor(ISensor sensor) { }
        public void VisitParameter(IParameter parameter) { }
    }

    /// <summary>One fan: an RPM sensor, a control, or both.</summary>
    class Fan
    {
        public string Id;
        public string Hw;
        public string Name;
        public ISensor Rpm;
        public ISensor Control;
        public bool Pump;
        public bool IsGpu;
    }

    static class Program
    {
        static Computer pc;
        static readonly object gate = new object();
        static readonly Dictionary<string, Fan> fans = new Dictionary<string, Fan>();
        static readonly Dictionary<string, float> requested = new Dictionary<string, float>();
        static readonly HashSet<string> touched = new HashSet<string>();

        /// <summary>A fan that follows a temperature curve.</summary>
        class Curve
        {
            public string Source = "hottest"; // cpu | gpu | hottest
            public List<(float t, float d)> Points = new List<(float t, float d)>();
            public float? LastTemp;
        }
        static readonly Dictionary<string, Curve> curves = new Dictionary<string, Curve>();

        /// <summary>"30:25,50:40,70:70,85:100" -> sorted points; null when it cannot be read.</summary>
        public static List<(float t, float d)> ParsePoints(string s)
        {
            var list = new List<(float t, float d)>();
            foreach (var p in (s ?? "").Split(new[] { ',' }, StringSplitOptions.RemoveEmptyEntries))
            {
                var td = p.Split(':');
                if (td.Length != 2) return null;
                if (!float.TryParse(td[0], NumberStyles.Float, CultureInfo.InvariantCulture, out var t)) return null;
                if (!float.TryParse(td[1], NumberStyles.Float, CultureInfo.InvariantCulture, out var d)) return null;
                if (float.IsNaN(t) || float.IsNaN(d)) return null;
                list.Add((t, Math.Max(0, Math.Min(100, d))));
            }
            if (list.Count == 0 || list.Count > 16) return null;
            list.Sort((a, b) => a.t.CompareTo(b.t));
            return list;
        }

        /// <summary>Move every curve-driven fan to its curve's duty for the current temperatures.</summary>
        static void RunCurves(float? cpu, float? gpu)
        {
            foreach (var kv in curves)
            {
                float? src = kv.Value.Source == "cpu" ? cpu : kv.Value.Source == "gpu" ? gpu : (cpu.HasValue || gpu.HasValue ? Math.Max(cpu ?? 0, gpu ?? 0) : (float?)null);
                // the chosen sensor is missing: follow whichever temperature is known
                if (!src.HasValue && (cpu.HasValue || gpu.HasValue)) src = Math.Max(cpu ?? 0, gpu ?? 0);
                if (!src.HasValue) continue; // no temperature at all: the "blind" rule hands the fan back
                float t = Safety.Hysteresis(kv.Value.LastTemp, src.Value);
                kv.Value.LastTemp = t;
                requested[kv.Key] = Safety.CurveDuty(kv.Value.Points, t);
            }
        }
        static readonly HashSet<string> pumpOverride = new HashSet<string>();
        static readonly HashSet<string> notPump = new HashSet<string>();
        static readonly ConcurrentQueue<string> inbox = new ConcurrentQueue<string>();
        // Status lines go out through their own thread: if PC Tuner stops reading, the pipe fills up and
        // only that thread blocks - the safety loop (watchdog, 90 C override) keeps running.
        static readonly BlockingCollection<string> outbox = new BlockingCollection<string>(4);
        static volatile bool stdinClosed;
        static bool emergency;
        static bool resetDone;
        static string lastMessage = "";
        static DateTime lastFromApp = DateTime.UtcNow;
        static DateTime lastTemps = DateTime.UtcNow;

        // Windows console control events (close, logoff, shutdown) -> give fans back.
        delegate bool ConsoleCtrlHandler(int ctrlType);
        [DllImport("kernel32.dll")]
        static extern bool SetConsoleCtrlHandler(ConsoleCtrlHandler handler, bool add);
        static ConsoleCtrlHandler ctrlHandler; // kept alive for the GC

        static int Main(string[] args)
        {
            Thread.CurrentThread.CurrentCulture = CultureInfo.InvariantCulture;
            try { Console.OutputEncoding = new UTF8Encoding(false); } catch { }
            try
            {
                if (args.Contains("--selftest")) return SelfTest();
                if (args.Contains("--dump"))
                {
                    int i = Array.IndexOf(args, "--dump");
                    string path = i + 1 < args.Length ? args[i + 1] : DefaultReportPath();
                    return Dump(path);
                }
                int parent = 0;
                int p = Array.IndexOf(args, "--parent");
                if (p >= 0 && p + 1 < args.Length) int.TryParse(args[p + 1], out parent);
                return Serve(parent);
            }
            catch (Exception e)
            {
                ResetAll("error: " + e.Message);
                Console.Error.WriteLine(e);
                return 1;
            }
        }

        static string DefaultReportPath() =>
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "PCTuner", "fan-report.txt");

        // ------------------------------------------------------------------ hardware

        static void Open()
        {
            pc = new Computer
            {
                IsCpuEnabled = true,
                IsGpuEnabled = true,
                IsMotherboardEnabled = true,
                IsControllerEnabled = true,
                IsMemoryEnabled = false,
                IsStorageEnabled = false,
                IsNetworkEnabled = false,
                IsPsuEnabled = false,
                IsBatteryEnabled = false,
            };
            pc.Open();
            pc.Accept(new UpdateVisitor());
            Discover();
        }

        static IEnumerable<IHardware> AllHardware()
        {
            var stack = new Stack<IHardware>(pc.Hardware.Reverse());
            while (stack.Count > 0)
            {
                var h = stack.Pop();
                yield return h;
                foreach (var s in h.SubHardware.Reverse()) stack.Push(s);
            }
        }

        static bool IsGpu(IHardware h) =>
            h.HardwareType == HardwareType.GpuNvidia || h.HardwareType == HardwareType.GpuAmd || h.HardwareType == HardwareType.GpuIntel;

        /// <summary>Pair every control with its RPM sensor (same hardware; same name, else same index).</summary>
        static void Discover()
        {
            fans.Clear();
            foreach (var h in AllHardware())
            {
                var rpms = h.Sensors.Where(s => s.SensorType == SensorType.Fan).ToList();
                var ctrls = h.Sensors.Where(s => s.SensorType == SensorType.Control && s.Control != null).ToList();
                var usedRpm = new HashSet<ISensor>();
                foreach (var c in ctrls)
                {
                    var r = rpms.FirstOrDefault(x => !usedRpm.Contains(x) && x.Name == c.Name)
                         ?? rpms.FirstOrDefault(x => !usedRpm.Contains(x) && x.Index == c.Index);
                    if (r != null) usedRpm.Add(r);
                    var f = new Fan { Id = c.Identifier.ToString(), Hw = h.Name, Name = c.Name, Rpm = r, Control = c, IsGpu = IsGpu(h) };
                    f.Pump = Safety.IsPumpName(c.Name) || (r != null && Safety.IsPumpName(r.Name));
                    fans[f.Id] = f;
                }
                foreach (var r in rpms.Where(x => !usedRpm.Contains(x)))
                {
                    var f = new Fan { Id = r.Identifier.ToString(), Hw = h.Name, Name = r.Name, Rpm = r, IsGpu = IsGpu(h) };
                    f.Pump = Safety.IsPumpName(r.Name);
                    fans[f.Id] = f;
                }
            }
        }

        static bool IsPump(Fan f) => !notPump.Contains(f.Id) && (f.Pump || pumpOverride.Contains(f.Id));

        static (float? cpu, float? gpu) KeyTemps()
        {
            float? cpu = null, gpu = null;
            foreach (var h in AllHardware())
            {
                var temps = h.Sensors.Where(s => s.SensorType == SensorType.Temperature && s.Value.HasValue && s.Value.Value > 0 && s.Value.Value < 150).ToList();
                if (temps.Count == 0) continue;
                if (h.HardwareType == HardwareType.Cpu)
                {
                    var main = temps.Where(s => s.Name.Contains("Tctl") || s.Name.Contains("Tdie") || s.Name.Contains("Package")).ToList();
                    float v = (main.Count > 0 ? main : temps).Max(s => s.Value.Value);
                    cpu = Math.Max(cpu ?? 0, v);
                }
                else if (IsGpu(h))
                {
                    var core = temps.FirstOrDefault(s => s.Name == "GPU Core") ?? temps[0];
                    gpu = Math.Max(gpu ?? 0, core.Value.Value);
                }
            }
            return (cpu, gpu);
        }

        // ------------------------------------------------------------------ control

        static void ApplyAll()
        {
            foreach (var kv in requested)
            {
                if (!fans.TryGetValue(kv.Key, out var f) || f.Control?.Control == null) continue;
                float duty = emergency ? 100f : Safety.Clamp(kv.Value, IsPump(f));
                var ctl = f.Control.Control;
                duty = Math.Max(ctl.MinSoftwareValue, Math.Min(ctl.MaxSoftwareValue, duty));
                ctl.SetSoftware(duty);
                touched.Add(f.Id);
            }
        }

        static void GiveBack(string id)
        {
            requested.Remove(id);
            curves.Remove(id);
            if (fans.TryGetValue(id, out var f) && f.Control?.Control != null)
            {
                try { f.Control.Control.SetDefault(); } catch { }
            }
            touched.Remove(id);
        }

        /// <summary>Every fan this helper touched goes back to BIOS / driver control. Safe to call more than once.</summary>
        static void ResetAll(string why)
        {
            lock (gate)
            {
                if (resetDone || pc == null) return;
                resetDone = true;
                foreach (var id in touched.ToList())
                {
                    if (fans.TryGetValue(id, out var f) && f.Control?.Control != null)
                    {
                        try { f.Control.Control.SetDefault(); } catch { }
                    }
                }
                touched.Clear();
                requested.Clear();
                curves.Clear();
                try { pc.Close(); } catch { }
                try { Console.Error.WriteLine("fans returned to BIOS: " + why); } catch { }
            }
        }

        static void Handle(string line)
        {
            var parts = line.Trim('﻿', ' ', '\t', '\r', '\n').Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
            if (parts.Length == 0) return;
            switch (parts[0])
            {
                case "ping":
                    break;
                case "set" when parts.Length >= 3:
                    if (!fans.TryGetValue(parts[1], out var f) || f.Control?.Control == null) { lastMessage = "not controllable: " + parts[1]; break; }
                    if (!float.TryParse(parts[2], NumberStyles.Float, CultureInfo.InvariantCulture, out var pct)) { lastMessage = "bad percent: " + parts[2]; break; }
                    curves.Remove(parts[1]);
                    requested[parts[1]] = pct;
                    break;
                case "curve" when parts.Length >= 4:
                    // curve <id> <cpu|gpu|hottest> t:d,t:d,...
                    if (!fans.TryGetValue(parts[1], out var cf) || cf.Control?.Control == null) { lastMessage = "not controllable: " + parts[1]; break; }
                    var pts = ParsePoints(parts[3]);
                    if (pts == null) { lastMessage = "bad curve: " + parts[3]; break; }
                    string src = parts[2] == "cpu" || parts[2] == "gpu" ? parts[2] : "hottest";
                    if (!curves.TryGetValue(parts[1], out var cur)) curves[parts[1]] = cur = new Curve();
                    cur.Source = src;
                    cur.Points = pts;
                    break;
                case "auto" when parts.Length >= 2:
                    GiveBack(parts[1]);
                    break;
                case "autoall":
                    foreach (var id in touched.Concat(requested.Keys).Distinct().ToList()) GiveBack(id);
                    break;
                case "pump" when parts.Length >= 3:
                    if (parts[2] == "1") { pumpOverride.Add(parts[1]); notPump.Remove(parts[1]); }
                    else { pumpOverride.Remove(parts[1]); if (fans.TryGetValue(parts[1], out var pf) && pf.Pump) notPump.Add(parts[1]); }
                    break;
                case "quit":
                    throw new QuitException("PC Tuner closed");
                default:
                    lastMessage = "unknown command: " + parts[0];
                    break;
            }
        }

        class QuitException : Exception { public QuitException(string m) : base(m) { } }

        static int Serve(int parentPid)
        {
            AppDomain.CurrentDomain.ProcessExit += (s, e) => ResetAll("exit");
            AppDomain.CurrentDomain.UnhandledException += (s, e) => ResetAll("crash");
            Console.CancelKeyPress += (s, e) => ResetAll("ctrl-c");
            ctrlHandler = t => { ResetAll("console event " + t); return false; };
            SetConsoleCtrlHandler(ctrlHandler, true);

            Process parent = null;
            if (parentPid > 0) { try { parent = Process.GetProcessById(parentPid); } catch { return 0; } }

            Open();

            var reader = new Thread(() =>
            {
                try
                {
                    // UTF-8 both ways; a byte-order mark at the start is detected and skipped
                    using (var input = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false), true))
                    {
                        string l;
                        while ((l = input.ReadLine()) != null) inbox.Enqueue(l);
                    }
                }
                catch { }
                stdinClosed = true;
            }) { IsBackground = true };
            reader.Start();
            var writer = new Thread(() =>
            {
                try
                {
                    foreach (var line in outbox.GetConsumingEnumerable())
                    {
                        Console.Out.WriteLine(line);
                        Console.Out.Flush();
                    }
                }
                catch { }
            }) { IsBackground = true };
            writer.Start();

            var visitor = new UpdateVisitor();
            string reason = "stopped";
            try
            {
                while (true)
                {
                    lock (gate)
                    {
                        while (inbox.TryDequeue(out var line)) { lastFromApp = DateTime.UtcNow; Handle(line); }
                        if (stdinClosed) { reason = "PC Tuner closed or crashed"; break; }
                        if (parent != null && parent.HasExited) { reason = "PC Tuner process ended"; break; }
                        if ((DateTime.UtcNow - lastFromApp).TotalSeconds > Safety.WatchdogSeconds) { reason = "PC Tuner stopped responding"; break; }

                        pc.Accept(visitor);
                        var (cpu, gpu) = KeyTemps();
                        if (cpu.HasValue || gpu.HasValue) lastTemps = DateTime.UtcNow;
                        emergency = Safety.Emergency(emergency, cpu, gpu);

                        RunCurves(cpu, gpu);
                        bool blind = (DateTime.UtcNow - lastTemps).TotalSeconds > Safety.BlindSeconds;
                        if (blind && requested.Count > 0)
                        {
                            foreach (var id in requested.Keys.ToList()) GiveBack(id);
                            lastMessage = "Temperatures could not be read - fans were given back to the BIOS.";
                        }
                        try { ApplyAll(); } catch (Exception e) { lastMessage = "control failed: " + e.Message; }

                        // never blocks: when the queue is full (nobody reading) this status is dropped
                        outbox.TryAdd(Status(cpu, gpu));
                        lastMessage = "";
                    }
                    Thread.Sleep(1000);
                }
            }
            catch (QuitException q) { reason = q.Message; }
            ResetAll(reason);
            return 0;
        }

        // ------------------------------------------------------------------ output

        static string Num(float? v) => v.HasValue && !float.IsNaN(v.Value) && !float.IsInfinity(v.Value) ? v.Value.ToString("0.#", CultureInfo.InvariantCulture) : "null";

        static string Str(string s)
        {
            var b = new StringBuilder("\"");
            foreach (char c in s ?? "")
            {
                switch (c)
                {
                    case '"': b.Append("\\\""); break;
                    case '\\': b.Append("\\\\"); break;
                    case '\n': b.Append("\\n"); break;
                    case '\r': b.Append("\\r"); break;
                    case '\t': b.Append("\\t"); break;
                    default:
                        if (c < 0x20) b.Append("\\u").Append(((int)c).ToString("x4")); else b.Append(c);
                        break;
                }
            }
            return b.Append('"').ToString();
        }

        static string Status(float? cpu, float? gpu)
        {
            var b = new StringBuilder();
            b.Append("{\"cpu\":").Append(Num(cpu)).Append(",\"gpu\":").Append(Num(gpu));
            b.Append(",\"emergency\":").Append(emergency ? "true" : "false");
            b.Append(",\"msg\":").Append(Str(lastMessage));
            b.Append(",\"fans\":[");
            bool first = true;
            foreach (var f in fans.Values)
            {
                if (!first) b.Append(',');
                first = false;
                var ctl = f.Control?.Control;
                string mode = ctl == null ? "none" : ctl.ControlMode == ControlMode.Software ? "software" : "default";
                b.Append("{\"id\":").Append(Str(f.Id))
                 .Append(",\"hw\":").Append(Str(f.Hw))
                 .Append(",\"name\":").Append(Str(f.Name))
                 .Append(",\"rpm\":").Append(Num(f.Rpm?.Value))
                 .Append(",\"percent\":").Append(Num(f.Control?.Value))
                 .Append(",\"controllable\":").Append(ctl != null ? "true" : "false")
                 .Append(",\"min\":").Append(ctl != null ? Num(ctl.MinSoftwareValue) : "null")
                 .Append(",\"mode\":").Append(Str(mode))
                 .Append(",\"pump\":").Append(IsPump(f) ? "true" : "false")
                 .Append(",\"gpu\":").Append(f.IsGpu ? "true" : "false")
                 .Append(",\"requested\":").Append(requested.TryGetValue(f.Id, out var r) ? Num(r) : "null")
                 .Append(",\"curve\":").Append(curves.ContainsKey(f.Id) ? "true" : "false")
                 .Append('}');
            }
            b.Append("],\"temps\":[");
            first = true;
            foreach (var h in AllHardware())
            {
                if (!(h.HardwareType == HardwareType.Cpu || IsGpu(h) || h.HardwareType == HardwareType.SuperIO || h.HardwareType == HardwareType.Motherboard || h.HardwareType == HardwareType.Cooler)) continue;
                foreach (var s in h.Sensors.Where(x => x.SensorType == SensorType.Temperature && x.Value.HasValue && x.Value.Value > 0 && x.Value.Value < 150))
                {
                    if (!first) b.Append(',');
                    first = false;
                    b.Append("{\"id\":").Append(Str(s.Identifier.ToString()))
                     .Append(",\"hw\":").Append(Str(h.Name))
                     .Append(",\"kind\":").Append(Str(h.HardwareType.ToString()))
                     .Append(",\"name\":").Append(Str(s.Name))
                     .Append(",\"value\":").Append(Num(s.Value))
                     .Append('}');
                }
            }
            b.Append("]}");
            return b.ToString();
        }

        static int Dump(string path)
        {
            Open();
            Thread.Sleep(1200);
            pc.Accept(new UpdateVisitor());
            var b = new StringBuilder();
            b.AppendLine("PC Tuner fan report  " + DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss"));
            b.AppendLine("Administrator: " + new System.Security.Principal.WindowsPrincipal(System.Security.Principal.WindowsIdentity.GetCurrent()).IsInRole(System.Security.Principal.WindowsBuiltInRole.Administrator));
            b.AppendLine();
            b.AppendLine("== Fans ==");
            foreach (var f in fans.Values)
            {
                var ctl = f.Control?.Control;
                b.AppendLine($"{f.Hw} | {f.Name} | rpm={Num(f.Rpm?.Value)} | percent={Num(f.Control?.Value)} | controllable={(ctl != null)}" +
                             (ctl != null ? $" | mode={ctl.ControlMode} | range={ctl.MinSoftwareValue}-{ctl.MaxSoftwareValue}" : "") +
                             $" | pump={f.Pump} | id={f.Id}");
            }
            var (cpu, gpu) = KeyTemps();
            b.AppendLine();
            b.AppendLine($"CPU temperature used for safety: {Num(cpu)}   GPU temperature used for safety: {Num(gpu)}");
            b.AppendLine();
            b.AppendLine("== Hardware and sensors ==");
            foreach (var h in AllHardware())
            {
                b.AppendLine($"[{h.HardwareType}] {h.Name}  ({h.Identifier})");
                foreach (var s in h.Sensors.Where(x => x.SensorType == SensorType.Temperature || x.SensorType == SensorType.Fan || x.SensorType == SensorType.Control))
                    b.AppendLine($"    {s.SensorType,-12} {s.Name,-28} {Num(s.Value),8}   {s.Identifier}");
            }
            pc.Close();
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            File.WriteAllText(path, b.ToString(), new UTF8Encoding(false));
            Console.Out.Write(b.ToString());
            return 0;
        }

        // ------------------------------------------------------------------ self-test (no hardware)

        static int SelfTest()
        {
            int fails = 0;
            void Check(bool ok, string what) { Console.Out.WriteLine((ok ? "ok    " : "FAIL  ") + what); if (!ok) fails++; }
            Check(Safety.Clamp(5, false) == 20, "fan floor 20 %");
            Check(Safety.Clamp(5, true) == 50, "pump floor 50 %");
            Check(Safety.Clamp(150, false) == 100, "max 100 %");
            Check(Safety.Clamp(float.NaN, false) == 100, "NaN -> 100 %");
            Check(Safety.Clamp(65, false) == 65, "normal value kept");
            Check(Safety.Emergency(false, 89.9f, 60) == false, "89.9 C is not an emergency");
            Check(Safety.Emergency(false, 90f, 60) == true, "CPU 90 C -> emergency");
            Check(Safety.Emergency(false, 50, 91) == true, "GPU 91 C -> emergency");
            Check(Safety.Emergency(true, 85, 60) == true, "stays on above 80 C");
            Check(Safety.Emergency(true, 79, 70) == false, "off when both below 80 C");
            Check(Safety.Emergency(true, null, null) == true, "stays on when temperatures are unreadable");
            Check(Safety.Emergency(true, 79, null) == false, "off with only CPU known and cool");
            Check(Safety.IsPumpName("Pump Fan") && Safety.IsPumpName("AIO Pump") && !Safety.IsPumpName("CPU Fan"), "pump names");
            Check(Str("a\"b\\c\n") == "\"a\\\"b\\\\c\\n\"", "JSON escaping");
            var pts = ParsePoints("70:70,30:25,50:40,85:100");
            Check(pts != null && pts.Count == 4 && pts[0].t == 30, "curve points parsed and sorted");
            Check(ParsePoints("30:x") == null && ParsePoints("") == null && ParsePoints("30") == null, "bad curves rejected");
            Check(Safety.CurveDuty(pts, 20) == 25, "below the first point: first duty");
            Check(Safety.CurveDuty(pts, 40) == 32.5f, "between points: linear");
            Check(Safety.CurveDuty(pts, 95) == 100, "above the last point: last duty");
            Check(Safety.Hysteresis(null, 60) == 60, "hysteresis starts at the current temperature");
            Check(Safety.Hysteresis(60, 65) == 65, "hysteresis follows rises at once");
            Check(Safety.Hysteresis(65, 63) == 65, "hysteresis ignores a 2 C drop");
            Check(Safety.Hysteresis(65, 62) == 62, "hysteresis follows a 3 C drop");
            Console.Out.WriteLine(fails == 0 ? "selftest passed" : $"selftest FAILED ({fails})");
            return fails == 0 ? 0 : 1;
        }
    }
}
