// PCTunerFan - the hardware side of PC Tuner's Fan Tuner tab.
//
// Reads temperatures and fans through LibreHardwareMonitorLib (which talks to the PawnIO driver)
// and drives fan headers on request. PC Tuner starts it hidden and talks over stdin/stdout:
//   helper -> app : one JSON status line per second
//   app -> helper : ping | set <id> <percent> | curve <id> <cpu|gpu|hottest> t:d,... | auto <id> | autoall
//                   | pump <id> 0|1 | quit
//
// Safety rules live here, so they hold even if the PC Tuner window freezes or crashes:
//   - fans never below 20 %, pumps never below 50 %
//   - any CPU or GPU temperature >= 90 C: every controlled fan to 100 % until both are below 80 C
//   - if temperatures cannot be read for 5 s while fans are controlled: fans go back to the BIOS until
//     they can be read again (the settings are kept and come back by themselves; status "blind":true)
//   - fans go back to the BIOS (SetDefault) on quit, when PC Tuner closes or crashes (stdin closes
//     or its process disappears), when PC Tuner sends nothing for 10 s, and on any error or shutdown.
//   - every helper starts with a reset pass right after the hardware opens, before it reads a command:
//     a control in software mode goes back to the BIOS (--reclaim: every controllable control, for fans
//     an earlier helper may have left behind). It reports "reset pass done: <g> gpu, <m> unconfirmed".
//     Only graphics-card fans can really be handed back by a new process: LibreHardwareMonitor restores a
//     motherboard (Super I/O) header only to the state it saved in the same process, so a header an earlier
//     (killed) helper left in manual mode stays there until a reboot - those count as "unconfirmed".
//   - --reclaim: the saved default of a motherboard header may be the dead helper's manual duty. Such a
//     header is never "handed back" (that would restore the stale duty): it goes to 100 % instead, and on
//     exit it is left at 100 % (pc.Close() is skipped, it would restore the stale duty). The exit line then
//     says "fans left at 100 %", never "fans returned to BIOS".
//
// Other modes: --reset-only runs just that reset pass on every control and exits; --dump [file] writes a
// sensor report; --selftest checks the safety logic (no hardware).

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

    /// <summary>The fan hardware as the controller sees it (the real one wraps LibreHardwareMonitor; --selftest uses a fake).</summary>
    public interface IFanIO
    {
        bool Controllable(string id);
        /// <summary>The header's name says it is a pump.</summary>
        bool NamedPump(string id);
        /// <summary>Drive the fan at this duty (already floored by the controller).</summary>
        void SetSoftware(string id, float duty);
        /// <summary>Hand the fan back to the BIOS / graphics driver.</summary>
        void SetDefault(string id);
        /// <summary>SetDefault works without an earlier SetSoftware in this process (graphics-card fans: the
        /// driver takes them back). A motherboard header is only restored to what this process saved.</summary>
        bool DriverReset(string id);
    }

    public class QuitException : Exception { public QuitException(string m) : base(m) { } }

    /// <summary>
    /// The safety loop: commands from PC Tuner, curves, floors, the 90 C override, the "blind" rule and the
    /// watchdog. It has no hardware or clock of its own, so --selftest drives it with fakes.
    /// </summary>
    public class Controller
    {
        readonly IFanIO io;
        readonly Dictionary<string, float> requested = new Dictionary<string, float>();
        readonly HashSet<string> touched = new HashSet<string>();
        readonly HashSet<string> pumpOverride = new HashSet<string>();
        readonly HashSet<string> notPump = new HashSet<string>();

        /// <summary>A fan that follows a temperature curve.</summary>
        class Curve
        {
            public string Source = "hottest"; // cpu | gpu | hottest
            public List<(float t, float d)> Points = new List<(float t, float d)>();
            public float? LastTemp;
        }
        readonly Dictionary<string, Curve> curves = new Dictionary<string, Curve>();

        DateTime lastFromApp;
        DateTime lastTemps;

        public bool Emergency { get; private set; }
        /// <summary>Temperatures unreadable for more than 5 s: the fans are on the BIOS until they can be read
        /// again. The requested settings are kept and come back by themselves.</summary>
        public bool Blind { get; private set; }
        /// <summary>The latest notice for PC Tuner; it stays until a new one replaces it.</summary>
        public string Message { get; private set; } = "";
        /// <summary>Goes up by one for every new notice.</summary>
        public int MessageSeq { get; private set; }

        /// <summary>--reclaim: an earlier helper may have left motherboard headers in manual mode, so what this
        /// process saves as their default may be that stale duty.</summary>
        readonly bool staleDefaults;

        public Controller(IFanIO io, DateTime now, bool staleDefaults = false)
        {
            this.io = io;
            lastFromApp = now;
            lastTemps = now;
            this.staleDefaults = staleDefaults;
        }

        /// <summary>A fan this helper drove cannot be handed back (stale default): it goes to 100 % instead
        /// and stays there after the helper exits, until a reboot.</summary>
        public bool Stranded { get; private set; }

        /// <summary>SetDefault would not give this fan back to the BIOS (its saved default may be stale).</summary>
        bool CannotHandBack(string id) => staleDefaults && !io.DriverReset(id);

        void Say(string m)
        {
            if (m == Message) return;
            Message = m;
            MessageSeq++;
        }

        public float? Requested(string id) => requested.TryGetValue(id, out var r) ? r : (float?)null;
        public bool HasCurve(string id) => curves.ContainsKey(id);
        public bool IsPump(string id) => !notPump.Contains(id) && (io.NamedPump(id) || pumpOverride.Contains(id));
        public IEnumerable<string> Touched => touched;

        /// <summary>"30:25,50:40,70:70,85:100" -> points sorted by temperature (a stable sort: equal temperatures
        /// keep their order, like PC Tuner's own curve code); null when it cannot be read.</summary>
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
            return list.OrderBy(p => p.t).ToList();
        }

        /// <summary>Give a fan back to the BIOS - or, when that is not possible (stale default), full speed.</summary>
        void SafeDefault(string id)
        {
            try
            {
                if (CannotHandBack(id)) { io.SetSoftware(id, 100f); Stranded = true; }
                else io.SetDefault(id);
            }
            catch { }
        }

        void GiveBack(string id)
        {
            requested.Remove(id);
            curves.Remove(id);
            bool stale = CannotHandBack(id);
            if (io.Controllable(id) && touched.Contains(id) && stale)
                Say("This fan cannot go back to the BIOS until you reboot - it runs at 100 %: " + id);
            // a stale-default fan this helper is not driving is left alone (never driven here, or already at 100 %)
            if (io.Controllable(id) && (touched.Contains(id) || !stale)) SafeDefault(id);
            touched.Remove(id);
        }

        /// <summary>One command from PC Tuner. Throws QuitException on "quit".</summary>
        public void Handle(string line, DateTime now)
        {
            lastFromApp = now;
            var parts = (line ?? "").Trim('﻿', ' ', '\t', '\r', '\n').Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
            if (parts.Length == 0) return;
            switch (parts[0])
            {
                case "ping":
                    break;
                case "set" when parts.Length >= 3:
                    if (!io.Controllable(parts[1])) { Say("not controllable: " + parts[1]); break; }
                    if (!float.TryParse(parts[2], NumberStyles.Float, CultureInfo.InvariantCulture, out var pct)) { Say("bad percent: " + parts[2]); break; }
                    curves.Remove(parts[1]);
                    requested[parts[1]] = pct;
                    break;
                case "curve" when parts.Length >= 4:
                    // curve <id> <cpu|gpu|hottest> t:d,t:d,...
                    if (!io.Controllable(parts[1])) { Say("not controllable: " + parts[1]); break; }
                    var pts = ParsePoints(parts[3]);
                    if (pts == null) { Say("bad curve: " + parts[3]); break; }
                    string src = parts[2] == "cpu" || parts[2] == "gpu" ? parts[2] : "hottest";
                    if (!curves.TryGetValue(parts[1], out var cur)) curves[parts[1]] = cur = new Curve();
                    cur.Source = src;
                    cur.Points = pts;
                    break;
                case "auto" when parts.Length >= 2:
                    GiveBack(parts[1]);
                    break;
                case "autoall":
                    foreach (var id in touched.Concat(requested.Keys).Concat(curves.Keys).Distinct().ToList()) GiveBack(id);
                    break;
                case "pump" when parts.Length >= 3:
                    if (parts[2] == "1") { pumpOverride.Add(parts[1]); notPump.Remove(parts[1]); }
                    else { pumpOverride.Remove(parts[1]); if (io.NamedPump(parts[1])) notPump.Add(parts[1]); }
                    break;
                case "quit":
                    throw new QuitException("PC Tuner closed");
                default:
                    Say("unknown command: " + parts[0]);
                    break;
            }
        }

        /// <summary>Move every curve-driven fan to its curve's duty for the current temperatures.</summary>
        void RunCurves(float? cpu, float? gpu)
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

        void ApplyAll()
        {
            if (Blind) return; // paused: the BIOS has the fans until temperatures can be read again
            foreach (var kv in requested)
            {
                if (!io.Controllable(kv.Key)) continue;
                float duty = Emergency ? 100f : Safety.Clamp(kv.Value, IsPump(kv.Key));
                io.SetSoftware(kv.Key, duty);
                if (CannotHandBack(kv.Key)) Stranded = true;
                touched.Add(kv.Key);
            }
        }

        /// <summary>One second of the safety loop. Returns why the helper must stop (watchdog), else null.</summary>
        public string Tick(DateTime now, float? cpu, float? gpu)
        {
            if ((now - lastFromApp).TotalSeconds > Safety.WatchdogSeconds) return "PC Tuner stopped responding";
            if (cpu.HasValue || gpu.HasValue) lastTemps = now;
            Emergency = Safety.Emergency(Emergency, cpu, gpu);
            RunCurves(cpu, gpu);
            bool blind = (now - lastTemps).TotalSeconds > Safety.BlindSeconds;
            if (blind && !Blind)
            {
                Blind = true;
                bool controlling = touched.Count > 0 || requested.Count > 0 || curves.Count > 0;
                foreach (var id in touched.ToList()) SafeDefault(id);
                touched.Clear();
                if (controlling) Say("Temperatures could not be read - fans were given back to the BIOS until they can be read again.");
            }
            else if (!blind && Blind)
            {
                Blind = false;
                if (requested.Count > 0 || curves.Count > 0) Say("Temperatures can be read again - PC Tuner controls the fans again.");
            }
            try { ApplyAll(); } catch (Exception e) { Say("control failed: " + e.Message); }
            return null;
        }

        /// <summary>The start-up reset pass: every listed controllable fan goes back to BIOS / driver control
        /// (one that fails does not stop the others). Returns (graphics-card fans handed back, other fans that
        /// may still be where an earlier process left them: a new process cannot restore those).</summary>
        public static (int confirmed, int unconfirmed) HandBack(IFanIO io, IEnumerable<string> ids)
        {
            int g = 0, m = 0;
            foreach (var id in ids)
            {
                if (!io.Controllable(id)) continue;
                bool ok = false;
                try { io.SetDefault(id); ok = io.DriverReset(id); } catch { }
                if (ok) g++; else m++;
            }
            return (g, m);
        }

        /// <summary>Every fan this controller touched goes back to BIOS / driver control; every request is dropped.</summary>
        public void ResetAll()
        {
            foreach (var id in touched.ToList()) SafeDefault(id);
            touched.Clear();
            requested.Clear();
            curves.Clear();
        }
    }

    static class Program
    {
        static Computer pc;
        static readonly object gate = new object();
        static readonly Dictionary<string, Fan> fans = new Dictionary<string, Fan>();
        static Controller ctl;
        static readonly ConcurrentQueue<string> inbox = new ConcurrentQueue<string>();
        // Status lines go out through their own thread: if PC Tuner stops reading, the pipe fills up and
        // only that thread blocks - the safety loop (watchdog, 90 C override) keeps running.
        static readonly BlockingCollection<string> outbox = new BlockingCollection<string>(4);
        static volatile bool stdinClosed;
        static bool resetDone;

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
                return Serve(parent, args.Contains("--reclaim"), args.Contains("--reset-only"));
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

        /// <summary>The real fans, through LibreHardwareMonitor.</summary>
        class HardwareIO : IFanIO
        {
            static IControl Ctl(string id) => fans.TryGetValue(id, out var f) ? f.Control?.Control : null;
            public bool Controllable(string id) => Ctl(id) != null;
            public bool NamedPump(string id) => fans.TryGetValue(id, out var f) && f.Pump;
            public void SetSoftware(string id, float duty)
            {
                var c = Ctl(id);
                if (c == null) return;
                c.SetSoftware(Math.Max(c.MinSoftwareValue, Math.Min(c.MaxSoftwareValue, duty)));
            }
            public void SetDefault(string id) => Ctl(id)?.SetDefault();
            public bool DriverReset(string id) => fans.TryGetValue(id, out var f) && f.IsGpu;
        }

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

        /// <summary>Every fan this helper touched goes back to BIOS / driver control. Safe to call more than once.</summary>
        static void ResetAll(string why)
        {
            lock (gate)
            {
                if (resetDone || pc == null) return;
                resetDone = true;
                try { ctl?.ResetAll(); } catch { }
                if (ctl != null && ctl.Stranded)
                {
                    // pc.Close() would restore the stale "default" (an earlier helper's manual duty): skip it,
                    // those fans stay at 100 %. Graphics-card fans were handed back by ResetAll above.
                    try { Console.Error.WriteLine("fans left at 100 % (they cannot go back to the BIOS until a reboot): " + why); } catch { }
                    return;
                }
                try { pc.Close(); } catch { }
                try { Console.Error.WriteLine("fans returned to BIOS: " + why); } catch { }
            }
        }

        /// <summary>Controls for the start-up reset pass: those in software mode, or every control (all).</summary>
        static List<string> ResetPassIds(bool all) =>
            fans.Values.Where(f => f.Control?.Control != null && (all || f.Control.Control.ControlMode == ControlMode.Software))
                       .Select(f => f.Id).ToList();

        static int Serve(int parentPid, bool reclaim, bool resetOnly)
        {
            AppDomain.CurrentDomain.ProcessExit += (s, e) => ResetAll("exit");
            AppDomain.CurrentDomain.UnhandledException += (s, e) => ResetAll("crash");
            Console.CancelKeyPress += (s, e) => ResetAll("ctrl-c");
            ctrlHandler = t => { ResetAll("console event " + t); return false; };
            SetConsoleCtrlHandler(ctrlHandler, true);

            Process parent = null;
            if (parentPid > 0) { try { parent = Process.GetProcessById(parentPid); } catch { return 0; } }

            Open();
            // reset pass before any command is read: a restarted helper hands back what an earlier one left
            lock (gate)
            {
                var (g, m) = Controller.HandBack(new HardwareIO(), ResetPassIds(reclaim || resetOnly));
                try { Console.Error.WriteLine($"reset pass done: {g} gpu, {m} unconfirmed"); Console.Error.Flush(); } catch { }
            }
            if (resetOnly) { ResetAll("reset pass only"); return 0; }
            lock (gate) ctl = new Controller(new HardwareIO(), DateTime.UtcNow, reclaim);

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
                        while (inbox.TryDequeue(out var line)) ctl.Handle(line, DateTime.UtcNow);
                        if (stdinClosed) { reason = "PC Tuner closed or crashed"; break; }
                        if (parent != null && parent.HasExited) { reason = "PC Tuner process ended"; break; }

                        pc.Accept(visitor);
                        var (cpu, gpu) = KeyTemps();
                        // floors, 90 C override, blind rule, watchdog
                        var stop = ctl.Tick(DateTime.UtcNow, cpu, gpu);
                        if (stop != null) { reason = stop; break; }

                        // never blocks: when the queue is full (nobody reading) this status is dropped
                        outbox.TryAdd(Status(cpu, gpu));
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
            b.Append(",\"emergency\":").Append(ctl.Emergency ? "true" : "false");
            b.Append(",\"blind\":").Append(ctl.Blind ? "true" : "false");
            b.Append(",\"msg\":").Append(Str(ctl.Message));
            b.Append(",\"msgSeq\":").Append(ctl.MessageSeq.ToString(CultureInfo.InvariantCulture));
            b.Append(",\"fans\":[");
            bool first = true;
            foreach (var f in fans.Values)
            {
                if (!first) b.Append(',');
                first = false;
                var fc = f.Control?.Control;
                string mode = fc == null ? "none" : fc.ControlMode == ControlMode.Software ? "software" : "default";
                b.Append("{\"id\":").Append(Str(f.Id))
                 .Append(",\"hw\":").Append(Str(f.Hw))
                 .Append(",\"name\":").Append(Str(f.Name))
                 .Append(",\"rpm\":").Append(Num(f.Rpm?.Value))
                 .Append(",\"percent\":").Append(Num(f.Control?.Value))
                 .Append(",\"controllable\":").Append(fc != null ? "true" : "false")
                 .Append(",\"min\":").Append(fc != null ? Num(fc.MinSoftwareValue) : "null")
                 .Append(",\"mode\":").Append(Str(mode))
                 .Append(",\"pump\":").Append(ctl.IsPump(f.Id) ? "true" : "false")
                 .Append(",\"gpu\":").Append(f.IsGpu ? "true" : "false")
                 .Append(",\"requested\":").Append(Num(ctl.Requested(f.Id)))
                 .Append(",\"curve\":").Append(ctl.HasCurve(f.Id) ? "true" : "false")
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
            var pts = Controller.ParsePoints("70:70,30:25,50:40,85:100");
            Check(pts != null && pts.Count == 4 && pts[0].t == 30, "curve points parsed and sorted");
            Check(Controller.ParsePoints("30:x") == null && Controller.ParsePoints("") == null && Controller.ParsePoints("30") == null, "bad curves rejected");
            var dup = Controller.ParsePoints("28:82,28:54");
            Check(dup != null && dup[0].d == 82 && dup[1].d == 54, "equal temperatures keep their order (stable sort, same as PC Tuner)");
            Check(Safety.CurveDuty(dup, 20) == 82 && Safety.CurveDuty(dup, 40) == 54, "duplicate-temperature curve duty matches PC Tuner");
            Check(Safety.CurveDuty(pts, 20) == 25, "below the first point: first duty");
            Check(Safety.CurveDuty(pts, 40) == 32.5f, "between points: linear");
            Check(Safety.CurveDuty(pts, 95) == 100, "above the last point: last duty");
            Check(Safety.Hysteresis(null, 60) == 60, "hysteresis starts at the current temperature");
            Check(Safety.Hysteresis(60, 65) == 65, "hysteresis follows rises at once");
            Check(Safety.Hysteresis(65, 63) == 65, "hysteresis ignores a 2 C drop");
            Check(Safety.Hysteresis(65, 62) == 62, "hysteresis follows a 3 C drop");
            ControllerTests(Check);
            Console.Out.WriteLine(fails == 0 ? "selftest passed" : $"selftest FAILED ({fails})");
            return fails == 0 ? 0 : 1;
        }

        /// <summary>Fake fans for the controller tests: records what the controller asked for. Models
        /// LibreHardwareMonitor: a motherboard header's SetDefault restores the state saved by this process's
        /// first SetSoftware (a no-op without one); a GPU fan (Gpu) always goes back to the driver.</summary>
        class FakeIO : IFanIO
        {
            public readonly HashSet<string> Ctl = new HashSet<string>();
            public readonly HashSet<string> Pumps = new HashSet<string>();
            public readonly HashSet<string> Gpu = new HashSet<string>();
            public readonly Dictionary<string, float> Duty = new Dictionary<string, float>(); // fans under software control
            public readonly HashSet<string> Defaulted = new HashSet<string>();
            readonly Dictionary<string, float?> saved = new Dictionary<string, float?>(); // null: it was on the BIOS
            public bool Controllable(string id) => Ctl.Contains(id);
            public bool NamedPump(string id) => Pumps.Contains(id);
            public bool DriverReset(string id) => Gpu.Contains(id);
            public void SetSoftware(string id, float duty)
            {
                if (!saved.ContainsKey(id)) saved[id] = Duty.TryGetValue(id, out var was) ? was : (float?)null;
                Duty[id] = duty;
                Defaulted.Remove(id);
            }
            public void SetDefault(string id)
            {
                float? back = null;
                if (!Gpu.Contains(id))
                {
                    if (!saved.TryGetValue(id, out back)) return; // nothing saved in this process: nothing is written
                    saved.Remove(id);
                }
                if (back.HasValue) { Duty[id] = back.Value; return; }
                Duty.Remove(id);
                Defaulted.Add(id);
            }
        }

        /// <summary>The safety loop with fake fans and a fake clock: floors, 90 C override, blind rule, watchdog, reset.</summary>
        static void ControllerTests(Action<bool, string> Check)
        {
            var t0 = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc);
            DateTime At(double s) => t0.AddSeconds(s);
            float? D(FakeIO f, string id) => f.Duty.TryGetValue(id, out var v) ? v : (float?)null;

            var io = new FakeIO();
            io.Ctl.UnionWith(new[] { "fan", "pump", "other" });
            io.Pumps.Add("pump");
            var c = new Controller(io, t0);
            c.Handle("set fan 5", At(0));
            c.Handle("set pump 5", At(0));
            c.Handle("set nope 50", At(0));
            Check(c.Tick(At(1), 50, 50) == null, "controller: a normal tick keeps running");
            Check(D(io, "fan") == 20, "controller: requested 5 % -> 20 % on a fan");
            Check(D(io, "pump") == 50, "controller: requested 5 % -> 50 % on a pump");
            Check(c.Message == "not controllable: nope" && c.MessageSeq == 1, "controller: an unknown fan is refused with a notice");
            Check(c.Tick(At(2), 50, 50) == null && c.MessageSeq == 1 && c.Message != "", "controller: the notice stays (not cleared every tick)");
            c.Handle("pump fan 1", At(2));
            c.Handle("pump pump 0", At(2));
            c.Tick(At(3), 50, 50);
            Check(D(io, "fan") == 50 && D(io, "pump") == 20, "controller: pump ticks move the floor both ways");
            c.Handle("pump fan 0", At(3));
            c.Handle("pump pump 1", At(3));
            c.Handle("set fan 40", At(3));
            c.Handle("set pump 60", At(3));
            c.Tick(At(4), 90, 50);
            Check(c.Emergency && D(io, "fan") == 100 && D(io, "pump") == 100, "controller: 90 C -> every controlled fan 100 %");
            c.Tick(At(5), 85, 50);
            Check(c.Emergency && D(io, "fan") == 100, "controller: stays at 100 % above 80 C");
            c.Tick(At(6), 79, 70);
            Check(!c.Emergency && D(io, "fan") == 40 && D(io, "pump") == 60, "controller: below 80 C on both -> back to the requested duties");
            c.Handle("curve other cpu 28:82,28:54", At(6));
            c.Tick(At(7), 20, 50);
            Check(D(io, "other") == 82 && c.HasCurve("other"), "controller: a curve drives its fan");

            // blind: no temperature for more than 5 s -> BIOS, settings kept, back when temperatures return
            c.Handle("ping", At(8));
            c.Tick(At(10), null, null);
            Check(!c.Blind && D(io, "fan") == 40, "controller: 3 s without temperatures is not blind yet");
            c.Handle("ping", At(12));
            int seq = c.MessageSeq;
            c.Tick(At(13), null, null);
            Check(c.Blind && io.Duty.Count == 0 && io.Defaulted.SetEquals(new[] { "fan", "pump", "other" }), "controller: blind for > 5 s -> every fan back to the BIOS");
            Check(c.MessageSeq == seq + 1 && c.Message.Contains("could not be read"), "controller: blind raises a notice");
            Check(c.Requested("fan") == 40 && c.HasCurve("other"), "controller: blind keeps the settings");
            c.Tick(At(14), null, null);
            Check(io.Duty.Count == 0, "controller: nothing is driven while blind");
            c.Tick(At(15), 50, null);
            Check(!c.Blind && D(io, "fan") == 40 && D(io, "pump") == 60 && D(io, "other") == 54, "controller: temperatures back -> the settings come back");
            Check(c.Message.Contains("again"), "controller: and a notice says so");

            // watchdog: nothing from PC Tuner for more than 10 s
            Check(c.Tick(At(22), 50, 50) == null, "controller: 10 s without a message is still fine");
            var why = c.Tick(At(22.5), 50, 50);
            Check(why != null, "controller: watchdog stops the helper after 10 s of silence");
            c.ResetAll();
            Check(io.Duty.Count == 0 && io.Defaulted.SetEquals(new[] { "fan", "pump", "other" }) && c.Requested("fan") == null && !c.HasCurve("other"),
                  "controller: ResetAll hands every touched fan back and forgets the settings");

            // auto / autoall / quit
            var io2 = new FakeIO();
            io2.Ctl.UnionWith(new[] { "a", "b" });
            var c2 = new Controller(io2, t0);
            c2.Handle("set a 40", At(0));
            c2.Handle("curve b gpu 30:30,80:100", At(0));
            c2.Tick(At(1), 50, 50);
            c2.Handle("auto a", At(1));
            Check(io2.Defaulted.Contains("a") && c2.Requested("a") == null, "controller: auto hands one fan back");
            c2.Handle("autoall", At(1));
            Check(io2.Duty.Count == 0 && !c2.HasCurve("b") && io2.Defaulted.Contains("b"), "controller: autoall hands every fan back");
            bool quit = false;
            try { c2.Handle("quit", At(2)); } catch (QuitException) { quit = true; }
            Check(quit, "controller: quit stops the helper");

            // start-up reset pass in a new process: an earlier (killed) helper left "x" (board) at 25 % and
            // "g" (graphics card) at 30 %. Only the GPU fan really goes back; board headers are unconfirmed.
            var io3 = new FakeIO();
            io3.Ctl.UnionWith(new[] { "x", "y", "g" });
            io3.Gpu.Add("g");
            io3.Duty["x"] = 25; io3.Duty["g"] = 30;
            var (gpuBack, unconfirmed) = Controller.HandBack(io3, new[] { "x", "y", "g", "gone" });
            Check(gpuBack == 1 && unconfirmed == 2 && io3.Defaulted.SetEquals(new[] { "g" }) && D(io3, "x") == 25,
                  "reset pass: only GPU fans count as handed back, board headers stay unconfirmed, unknown ids skipped");

            // why --reclaim must not "hand back" a board header: a normal controller saves the leftover 25 % as
            // the default and restores it
            var io4 = new FakeIO();
            io4.Ctl.Add("x");
            io4.Duty["x"] = 25;
            var c4 = new Controller(io4, t0);
            c4.Handle("set x 60", At(0));
            c4.Tick(At(1), 50, 50);
            c4.ResetAll();
            Check(D(io4, "x") == 25 && !c4.Stranded, "model: a new process restores a leftover header to its stale duty");

            // --reclaim (stale defaults): board headers are driven, never handed back to a stale duty
            var c3 = new Controller(io3, t0, true);
            c3.Handle("set x 40", At(0));
            c3.Handle("set g 40", At(0));
            c3.Tick(At(1), 50, 50);
            Check(D(io3, "x") == 40 && D(io3, "g") == 40 && c3.Stranded, "reclaim: leftover fans are driven again under the safety rules");
            c3.Tick(At(2), 91, 50);
            Check(D(io3, "x") == 100 && D(io3, "g") == 100, "reclaim: the 90 C override still holds");
            c3.Tick(At(3), 50, 50);
            c3.Handle("ping", At(5));
            c3.Tick(At(9), null, null);
            Check(c3.Blind && D(io3, "x") == 100 && io3.Defaulted.Contains("g"), "reclaim: blind -> board header 100 % (not its stale duty), GPU back to the driver");
            c3.Tick(At(10), 50, 50);
            Check(D(io3, "x") == 40 && D(io3, "g") == 40, "reclaim: temperatures back -> settings back");
            c3.Handle("auto x", At(10));
            Check(D(io3, "x") == 100 && c3.Message.Contains("reboot"), "reclaim: auto on a board header -> 100 % with a notice");
            c3.Handle("set x 40", At(10));
            c3.Tick(At(11), 50, 50);
            c3.ResetAll();
            Check(D(io3, "x") == 100 && io3.Defaulted.Contains("g") && c3.Stranded,
                  "reclaim: ResetAll leaves board headers at 100 % (stranded), GPU fans back to the driver");
            c3.Handle("auto y", At(11));
            Check(D(io3, "y") == null, "reclaim: auto on a board header this helper never drove leaves it alone");
            var io5 = new FakeIO();
            io5.Ctl.Add("g"); io5.Gpu.Add("g");
            var c5 = new Controller(io5, t0, true);
            c5.Handle("set g 40", At(0));
            c5.Tick(At(1), 50, 50);
            c5.ResetAll();
            Check(!c5.Stranded && io5.Defaulted.Contains("g"), "reclaim: GPU fans only -> nothing stranded");
        }
    }
}
