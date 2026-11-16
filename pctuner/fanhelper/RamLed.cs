// RAM LED control for PC Tuner's RAM LED tab: one-shot commands, one JSON line on stdout.
//
//   --ramled detect                                  read-only: which sticks have an ENE LED controller, and their state
//   --ramled off [--save]                            LEDs dark
//   --ramled static RRGGBB [--save]                  one fixed colour
//   --ramled rainbow [--save]                        the default rainbow effect
//   --ramled restore <id>:<mode>:<speed>:<dir>:<hex colours> ... [--save]   put a detect snapshot back
//
// --save also stores the setting in the stick's LED controller, so it survives a power cycle.
//
// The sticks are reached over the motherboard SMBus (PawnIO driver, through LibreHardwareMonitorLib and
// RAMSPDToolkit). The register protocol is the ENE DRAM one that OpenRGB's ENESMBusController documents.
//
// Safety rules (the same bus carries the SPD chips that hold the memory's timing data):
//   - only addresses 0x70-0x77 are ever touched; never the SPD hub (0x50-0x57) or its sensors (0x18-0x1F)
//   - no address-remap sequence (that is a DDR4 thing)
//   - nothing is written to an address until it has answered like an ENE LED controller and has a known name
//   - the global SMBus mutex is held, so HWiNFO / AIDA64 and this helper never interleave transfers

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using LibreHardwareMonitor.Hardware;
using RAMSPDToolkit.I2CSMBus;

namespace PCTunerFan
{
    /// <summary>The three SMBus transfers the ENE protocol needs. Results below zero are errors.</summary>
    public interface IEneBus
    {
        int ReadByteData(byte addr, byte cmd);
        int WriteByteData(byte addr, byte cmd, byte val);
        int WriteWordData(byte addr, byte cmd, ushort val);
    }

    /// <summary>One stick's LED controller and its state when it was read.</summary>
    public sealed class EneStick
    {
        public string Id;      // "<bus index>-<address hex>", stable for a given slot
        public string Bus;
        public byte Addr;
        public string Name;    // controller name string, e.g. AUMA0-E6K5-0107
        public int Leds;
        public bool ColorOk;   // the colour registers of this controller version are known
        public int Mode, Speed, Dir;
        public byte[] Colors = new byte[0]; // effect colours as stored (R,B,G per LED)
    }

    /// <summary>ENE DRAM LED controller at one SMBus address (pure logic over IEneBus; tested by --selftest).</summary>
    public sealed class EneDevice
    {
        public const byte FirstAddr = 0x70, LastAddr = 0x77;
        /// <summary>Where ASUS graphics cards keep their ENE LED controller, on the card's own I2C bus.</summary>
        public const byte GpuAddr = 0x67;
        public const int ModeOff = 0, ModeStatic = 1, ModeRainbow = 5;
        const ushort RegName = 0x1000, RegMicron = 0x1030, RegConfig = 0x1C00;
        const ushort RegEffectV1 = 0x8010, RegEffectV2 = 0x8160;
        const ushort RegDirect = 0x8020, RegMode = 0x8021, RegSpeed = 0x8022, RegDir = 0x8023, RegApply = 0x80A0;
        const byte ApplyVal = 0x01, SaveVal = 0xAA;
        const int MaxLeds = 16;

        readonly IEneBus bus;
        readonly byte addr;
        ushort effectReg; // 0: unknown controller version, colours are left alone

        EneDevice(IEneBus bus, byte addr, bool gpu)
        {
            // RAM: only 0x70-0x77 (the same bus carries the SPD chips); a graphics card: only 0x67
            if (gpu ? addr != GpuAddr : addr < FirstAddr || addr > LastAddr) throw new ArgumentException($"address 0x{addr:X2} is outside the LED controller range");
            this.bus = bus;
            this.addr = addr;
        }

        /// <summary>Controller versions whose colour registers are known (from OpenRGB). 0 = unknown.</summary>
        public static ushort EffectRegister(string name)
        {
            switch (name)
            {
                case "LED-0116":
                case "DIMM_LED-0102":
                case "DIMM_LED-0103":
                case "AUMA0-E8K4-0101":
                    return RegEffectV1;
                case "AUDA0-E6K5-0101":
                case "AUMA0-E6K5-0104":
                case "AUMA0-E6K5-0105":
                case "AUMA0-E6K5-0106":
                case "AUMA0-E6K5-0107":
                case "AUMA0-E6K5-1110":
                case "AUMA0-E6K5-1111":
                case "AUMA0-E6K5-1107":
                case "AUMA0-E6K5-0008":
                case "AUMA0-E6K5-1113":
                case "AUMA0-E6K5-1114":
                    return RegEffectV2;
                default:
                    return 0;
            }
        }

        /// <summary>Where the LED count sits in the config table (newer AUMA0 versions moved it).</summary>
        static int LedCountOffset(string name)
        {
            switch (name)
            {
                case "AUMA0-E6K5-0107":
                case "AUMA0-E6K5-1110":
                case "AUMA0-E6K5-1111":
                case "AUMA0-E6K5-1107":
                case "AUMA0-E6K5-0008":
                case "AUMA0-E6K5-1113":
                case "AUMA0-E6K5-1114":
                    return 0x03;
                default:
                    return 0x02;
            }
        }

        static bool KnownFamily(string name) =>
            name.StartsWith("LED-") || name.StartsWith("DIMM_LED-") || name.StartsWith("AUMA0-") || name.StartsWith("AUDA0-");

        /// <summary>Read-only until the address has answered with the ENE test pattern (only then is the
        /// register pointer set, which is the first thing written). Null: no supported controller here.</summary>
        public static EneDevice Probe(IEneBus bus, byte addr, out EneStick stick, bool gpu = false)
        {
            stick = null;
            var d = new EneDevice(bus, addr, gpu);
            for (int i = 0; i < 16; i++)
                if (bus.ReadByteData(addr, (byte)(0xA0 + i)) != i) return null;
            string name = d.ReadString(RegName, 16);
            if (!KnownFamily(name)) return null;
            if (d.ReadString(RegMicron, 6) == "Micron") return null;
            d.effectReg = EffectRegister(name);
            int leds = d.R((ushort)(RegConfig + LedCountOffset(name)));
            if (leds <= 0 || leds > MaxLeds) leds = 8;
            stick = new EneStick { Addr = addr, Name = name, Leds = leds, ColorOk = d.effectReg != 0 };
            d.ReadState(stick);
            return d;
        }

        public void ReadState(EneStick s)
        {
            s.Mode = R(RegMode);
            s.Speed = R(RegSpeed);
            s.Dir = R(RegDir);
            if (effectReg != 0)
            {
                var c = new byte[s.Leds * 3];
                for (int i = 0; i < c.Length; i++) c[i] = (byte)R((ushort)(effectReg + i));
                s.Colors = c;
            }
        }

        public void SetMode(int mode)
        {
            W(RegDirect, 0);
            W(RegMode, (byte)mode);
            W(RegApply, ApplyVal);
        }

        public void SetStatic(int leds, byte r, byte g, byte b)
        {
            if (effectReg == 0) throw new InvalidOperationException("colour is not supported on this LED controller version");
            for (int i = 0; i < Math.Min(leds, MaxLeds); i++)
            {
                W((ushort)(effectReg + i * 3), r);
                W((ushort)(effectReg + i * 3 + 1), b); // the controller stores R,B,G
                W((ushort)(effectReg + i * 3 + 2), g);
            }
            SetMode(ModeStatic);
        }

        /// <summary>Put a detect snapshot back (colours only when they were read from the same controller version).</summary>
        public void Restore(int mode, int speed, int dir, byte[] colors)
        {
            if (mode < 0 || mode > 14) throw new ArgumentException("bad mode");
            if (effectReg != 0 && colors != null)
                for (int i = 0; i < Math.Min(colors.Length, MaxLeds * 3); i++) W((ushort)(effectReg + i), colors[i]);
            W(RegDirect, 0);
            W(RegMode, (byte)mode);
            W(RegSpeed, (byte)speed);
            W(RegDir, (byte)dir);
            W(RegApply, ApplyVal);
        }

        /// <summary>Store the current setting in the controller so it survives a power cycle.</summary>
        public void Save() => W(RegApply, SaveVal);

        void Select(ushort reg)
        {
            // the register number goes on the wire high byte first
            ushort swapped = (ushort)(((reg << 8) & 0xFF00) | ((reg >> 8) & 0x00FF));
            if (bus.WriteWordData(addr, 0x00, swapped) < 0) throw new IOException($"SMBus error selecting register 0x{reg:X4} at 0x{addr:X2}");
        }

        int R(ushort reg)
        {
            Select(reg);
            int v = bus.ReadByteData(addr, 0x81);
            if (v < 0) throw new IOException($"SMBus error reading register 0x{reg:X4} at 0x{addr:X2}");
            return v & 0xFF;
        }

        void W(ushort reg, byte val)
        {
            Select(reg);
            if (bus.WriteByteData(addr, 0x01, val) < 0) throw new IOException($"SMBus error writing register 0x{reg:X4} at 0x{addr:X2}");
        }

        string ReadString(ushort reg, int len)
        {
            var b = new StringBuilder();
            for (int i = 0; i < len; i++)
            {
                int c = R((ushort)(reg + i));
                if (c == 0) break;
                if (c < 0x20 || c > 0x7E) return ""; // not text: not a controller we know
                b.Append((char)c);
            }
            return b.ToString();
        }
    }

    public static class RamLed
    {
        const string SmbusMutex = @"Global\Access_SMBUS.HTP.Method"; // shared with HWiNFO, AIDA64, OpenRGB...

        sealed class RealBus : IEneBus
        {
            readonly SMBusInterface b;
            public RealBus(SMBusInterface b) { this.b = b; }
            public int ReadByteData(byte addr, byte cmd) => b.i2c_smbus_read_byte_data(addr, cmd);
            public int WriteByteData(byte addr, byte cmd, byte val) => b.i2c_smbus_write_byte_data(addr, cmd, val);
            public int WriteWordData(byte addr, byte cmd, ushort val) => b.i2c_smbus_write_word_data(addr, cmd, val);
        }

        public static string Hex(byte[] b) => string.Concat((b ?? new byte[0]).Select(x => x.ToString("X2")));

        public static byte[] ParseHex(string s)
        {
            if (s == null || s.Length % 2 != 0) return null;
            var o = new byte[s.Length / 2];
            for (int i = 0; i < o.Length; i++)
                if (!byte.TryParse(s.Substring(i * 2, 2), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out o[i])) return null;
            return o;
        }

        static string Esc(string s) => "\"" + (s ?? "").Replace("\\", "\\\\").Replace("\"", "\\\"") + "\"";

        public static string Json(bool ok, string error, IEnumerable<EneStick> sticks)
        {
            var b = new StringBuilder();
            b.Append("{\"ok\":").Append(ok ? "true" : "false");
            b.Append(",\"error\":").Append(Esc(error));
            b.Append(",\"sticks\":[");
            bool first = true;
            foreach (var s in sticks ?? Enumerable.Empty<EneStick>())
            {
                if (!first) b.Append(',');
                first = false;
                b.Append("{\"id\":").Append(Esc(s.Id));
                b.Append(",\"bus\":").Append(Esc(s.Bus));
                b.Append(",\"addr\":").Append(s.Addr);
                b.Append(",\"name\":").Append(Esc(s.Name));
                b.Append(",\"leds\":").Append(s.Leds);
                b.Append(",\"color_ok\":").Append(s.ColorOk ? "true" : "false");
                b.Append(",\"mode\":").Append(s.Mode);
                b.Append(",\"speed\":").Append(s.Speed);
                b.Append(",\"dir\":").Append(s.Dir);
                b.Append(",\"colors\":").Append(Esc(Hex(s.Colors)));
                b.Append('}');
            }
            b.Append("]}");
            return b.ToString();
        }

        /// <summary>Probe 0x70-0x77 on one bus.</summary>
        public static List<(EneDevice dev, EneStick stick)> Scan(IEneBus bus, int busIndex, string busName)
        {
            var found = new List<(EneDevice, EneStick)>();
            for (byte a = EneDevice.FirstAddr; a <= EneDevice.LastAddr; a++)
            {
                var d = EneDevice.Probe(bus, a, out var s);
                if (d == null) continue;
                s.Id = $"{busIndex}-{a:x2}";
                s.Bus = busName;
                found.Add((d, s));
            }
            return found;
        }

        /// <summary>Run one command on the sticks found; returns the state read back afterwards.</summary>
        public static List<EneStick> Apply(List<(EneDevice dev, EneStick stick)> found, string cmd, string[] rest, bool save)
        {
            foreach (var (dev, stick) in found)
            {
                switch (cmd)
                {
                    case "detect":
                        continue;
                    case "off":
                        dev.SetMode(EneDevice.ModeOff);
                        break;
                    case "rainbow":
                        dev.SetMode(EneDevice.ModeRainbow);
                        break;
                    case "static":
                        var rgb = ParseHex(rest.FirstOrDefault());
                        if (rgb == null || rgb.Length != 3) throw new ArgumentException("static needs a colour like FF8000");
                        dev.SetStatic(stick.Leds, rgb[0], rgb[1], rgb[2]);
                        break;
                    case "restore":
                        string mine = rest.FirstOrDefault(r => r.StartsWith(stick.Id + ":"));
                        if (mine == null) continue; // no snapshot for this stick: leave it alone
                        var p = mine.Split(':');
                        if (p.Length != 5) throw new ArgumentException("bad snapshot");
                        dev.Restore(int.Parse(p[1]), int.Parse(p[2]), int.Parse(p[3]), ParseHex(p[4]));
                        break;
                    default:
                        throw new ArgumentException("unknown command " + cmd);
                }
                if (save) dev.Save();
                dev.ReadState(stick);
            }
            return found.Select(f => f.stick).ToList();
        }

        public static int Run(string[] args)
        {
            int i = Array.IndexOf(args, "--ramled");
            string cmd = i + 1 < args.Length ? args[i + 1] : "detect";
            bool save = args.Contains("--save");
            string[] rest = args.Skip(i + 2).Where(a => a != "--save").ToArray();
            Computer pc = null;
            Mutex mutex = null;
            bool held = false;
            try
            {
                // Memory hardware on: LibreHardwareMonitor loads the PawnIO SMBus module and registers the buses.
                pc = new Computer { IsMemoryEnabled = true };
                pc.Open();
                if (SMBusManager.RegisteredSMBuses.Count == 0) SMBusManager.DetectSMBuses();
                var buses = SMBusManager.RegisteredSMBuses
                    .Where(b => { string t = b.GetType().Name; return t == "SMBusPawnIO" || t == "SMBusI801" || t == "SMBusPiix4"; }) // motherboard buses only, never a graphics card's
                    .ToList();
                if (buses.Count == 0)
                {
                    Console.Out.WriteLine(Json(false, "No motherboard SMBus found. PC Tuner must run as administrator and the PawnIO driver must be installed.", null));
                    return 2;
                }
                try { mutex = new Mutex(false, SmbusMutex); } catch { mutex = null; }
                if (mutex != null)
                {
                    try { held = mutex.WaitOne(3000); } catch (AbandonedMutexException) { held = true; }
                    if (!held)
                    {
                        Console.Out.WriteLine(Json(false, "Another program is using the SMBus (hardware monitor or lighting software). Close it and try again.", null));
                        return 3;
                    }
                }
                var found = new List<(EneDevice, EneStick)>();
                for (int n = 0; n < buses.Count; n++)
                    found.AddRange(Scan(new RealBus(buses[n]), n, $"{buses[n].GetType().Name} {buses[n].DeviceName} port {buses[n].PortID}"));
                if (found.Count == 0)
                {
                    Console.Out.WriteLine(Json(false, "No supported RAM LED controller found.", null));
                    return 4;
                }
                var sticks = Apply(found, cmd, rest, save);
                Console.Out.WriteLine(Json(true, "", sticks));
                return 0;
            }
            catch (Exception e)
            {
                Console.Error.WriteLine(e);
                Console.Out.WriteLine(Json(false, e.Message, null));
                return 1;
            }
            finally
            {
                if (held) try { mutex.ReleaseMutex(); } catch { }
                mutex?.Dispose();
                try { pc?.Close(); } catch { }
            }
        }

        // ------------------------------------------------------------------ self-test (no hardware)

        /// <summary>A fake stick: ENE test pattern, a register pointer and memory; records every transfer that writes.</summary>
        sealed class FakeEne : IEneBus
        {
            public byte At = 0x71;
            public bool Pattern = true;
            public readonly Dictionary<ushort, byte> Mem = new Dictionary<ushort, byte>();
            public readonly List<string> Writes = new List<string>(); // "REG=VAL"
            public readonly List<ushort> Wire = new List<ushort>();   // register-select words as sent
            public int Transfers;                                      // everything that is not a plain read
            ushort ptr;
            public void Text(ushort reg, string s) { for (int i = 0; i < s.Length; i++) Mem[(ushort)(reg + i)] = (byte)s[i]; }
            public int ReadByteData(byte addr, byte cmd)
            {
                if (addr != At) return -1;
                if (cmd >= 0xA0 && cmd <= 0xAF) return Pattern ? cmd - 0xA0 : 0xFF;
                if (cmd == 0x81) return Mem.TryGetValue(ptr, out var v) ? v : 0;
                return 0;
            }
            public int WriteByteData(byte addr, byte cmd, byte val)
            {
                Transfers++;
                if (addr != At || cmd != 0x01) return -1;
                Mem[ptr] = val;
                Writes.Add($"{ptr:X4}={val:X2}");
                return 0;
            }
            public int WriteWordData(byte addr, byte cmd, ushort val)
            {
                Transfers++;
                if (addr != At || cmd != 0x00) return -1;
                Wire.Add(val);
                ptr = (ushort)(((val << 8) & 0xFF00) | ((val >> 8) & 0x00FF));
                return 0;
            }
        }

        public static void SelfTest(Action<bool, string> Check)
        {
            FakeEne Stick(string name)
            {
                var f = new FakeEne();
                f.Text(0x1000, name);
                f.Mem[0x1C02] = 8;
                f.Mem[0x1C03] = 8;
                f.Mem[0x8021] = 5;
                return f;
            }

            var f1 = Stick("AUMA0-E6K5-0107");
            var found = Scan(f1, 0, "fake");
            Check(found.Count == 1 && found[0].stick.Id == "0-71" && found[0].stick.Name == "AUMA0-E6K5-0107", "ram led: stick found at 0x71 with its name");
            Check(found[0].stick.Leds == 8 && found[0].stick.Mode == 5 && found[0].stick.ColorOk && found[0].stick.Colors.Length == 24, "ram led: state read (8 LEDs, rainbow)");
            Check(f1.Writes.Count == 0, "ram led: detect changes no register");
            Check(f1.Wire.Contains((ushort)0x0010), "ram led: register number goes on the wire high byte first");

            Apply(found, "off", new string[0], false);
            Check(f1.Mem[0x8021] == 0 && f1.Mem[0x8020] == 0 && f1.Writes.Last() == "80A0=01", "ram led: off = mode 0, then apply");
            Check(found[0].stick.Mode == 0, "ram led: state re-read after a change");
            Check(!f1.Writes.Contains("80A0=AA"), "ram led: nothing saved without --save");

            Apply(found, "static", new[] { "FF8040" }, true);
            Check(f1.Mem[0x8160] == 0xFF && f1.Mem[0x8161] == 0x40 && f1.Mem[0x8162] == 0x80, "ram led: colour stored as R,B,G");
            Check(f1.Mem[0x8160 + 21] == 0xFF && !f1.Mem.ContainsKey(0x8160 + 24), "ram led: exactly 8 LEDs written");
            Check(f1.Mem[0x8021] == 1 && f1.Writes.Last() == "80A0=AA", "ram led: static = mode 1, apply, then save");

            Apply(found, "rainbow", new string[0], false);
            Check(f1.Mem[0x8021] == 5, "ram led: rainbow = mode 5");

            Apply(found, "restore", new[] { "0-71:4:2:1:" + new string('0', 46) + "AB" }, false);
            Check(f1.Mem[0x8021] == 4 && f1.Mem[0x8022] == 2 && f1.Mem[0x8023] == 1 && f1.Mem[0x8160 + 23] == 0xAB, "ram led: snapshot restored");
            int before = f1.Writes.Count;
            Apply(found, "restore", new[] { "0-72:0:0:0:" }, false);
            Check(f1.Writes.Count == before, "ram led: a snapshot for another stick is not applied");

            var absent = Stick("AUMA0-E6K5-0107");
            absent.Pattern = false;
            Check(Scan(absent, 0, "fake").Count == 0 && absent.Transfers == 0, "ram led: no ENE pattern -> nothing found, nothing sent");
            var odd = Stick("SOMETHING-ELSE");
            Check(Scan(odd, 0, "fake").Count == 0 && odd.Writes.Count == 0, "ram led: unknown name -> ignored, no register changed");
            var micron = Stick("AUMA0-E6K5-0107");
            micron.Text(0x1030, "Micron");
            Check(Scan(micron, 0, "fake").Count == 0, "ram led: Micron controller excluded");

            var unk = Stick("AUMA0-E6K5-9999");
            var fu = Scan(unk, 0, "fake");
            bool threw = false;
            try { Apply(fu, "static", new[] { "FFFFFF" }, false); } catch (InvalidOperationException) { threw = true; }
            Check(fu.Count == 1 && !fu[0].stick.ColorOk && threw && unk.Writes.Count == 0, "ram led: unknown version -> colour refused, nothing written");
            Apply(fu, "off", new string[0], false);
            Check(unk.Mem[0x8021] == 0, "ram led: unknown version can still be switched off");

            bool guard = false;
            try { EneDevice.Probe(f1, 0x50, out _); } catch (ArgumentException) { guard = true; }
            Check(guard, "ram led: the SPD address 0x50 is refused");
            Check(Hex(ParseHex("00ffA1")) == "00FFA1" && ParseHex("abc") == null && ParseHex("zz") == null, "ram led: hex parsing");
            Check(Json(true, "", found.Select(x => x.stick)).Contains("\"id\":\"0-71\""), "ram led: JSON output");
        }
    }
}
