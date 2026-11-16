// Graphics card LED control for PC Tuner's RAM LED tab: the same one-shot commands and JSON as --ramled.
//
//   --gpuled detect | off | static RRGGBB | rainbow | restore <snapshot> ...  [--save]
//   --gpuled probe                                   read-only diagnostics on stderr
//
// The LED controller sits on the card's own I2C bus, reached through the NVIDIA driver (nvapi64.dll:
// NvAPI_I2CReadEx / NvAPI_I2CWriteEx on port 1 - the layout OpenRGB's i2c_smbus_nvapi uses). Two kinds:
//   - ENE controller at 0x67, the same register protocol as the RAM sticks (EneDevice): ASUS cards, and this
//     PC's Gigabyte RTX 5080 XTREME WATERFORCE (1458:418B), whose controller reads AUMA0-E6K5-1107
//   - fallback for that Gigabyte model: the RGB Fusion 2 "Blackwell" controller OpenRGB lists at 0x75
//     (64-byte packets, three zones) - not present on this PC's card
//
// Safety rules:
//   - only ASUS- and Gigabyte-made NVIDIA cards are looked at, and only address 0x67 (and 0x75 on the one model)
//   - ENE: nothing is written until 0x67 has answered with the ENE test pattern and a known controller name
//   - Fusion 2: only the identify packet is sent until the controller has answered like a Fusion 2 Blackwell one
//   - the Fusion 2 controller takes at most 9 mode packets a second: packets are spaced 150 ms apart

using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;

namespace PCTunerFan
{
    /// <summary>Gigabyte RGB Fusion 2 "Blackwell" GPU controller (pure packet logic; tested by --selftest).</summary>
    public sealed class Fusion2Gpu
    {
        public const byte Addr = 0x75;
        public const byte RegIdentify = 0x10, RegMode = 0x12, RegSave = 0x13;
        public const byte ModeStatic = 0x01, ModeColorCycle = 0x05;
        public const byte SpeedNormal = 0x03, BrightnessMax = 0x0A;
        /// <summary>Hardware zones of the XTREME WATERFORCE layout.</summary>
        public const int WaterforceZones = 3;

        public interface IRawBus
        {
            bool Write(byte addr, byte[] packet);
            byte[] Read(byte addr, int size);
        }

        readonly IRawBus bus;
        readonly int zones;
        readonly int delayMs;
        Fusion2Gpu(IRawBus bus, int zones, int delayMs) { this.bus = bus; this.zones = zones; this.delayMs = delayMs; }

        public static byte[] Packet(params byte[] head)
        {
            var p = new byte[64];
            Array.Copy(head, p, Math.Min(head.Length, 64));
            return p;
        }

        /// <summary>The identify answer: 01, then 01 or 02, then 01 (OpenRGB's test).</summary>
        public static bool IsFusion2(byte[] r) => r != null && r.Length >= 3 && r[0] == 0x01 && (r[1] == 0x01 || r[1] == 0x02) && r[2] == 0x01;

        /// <summary>Send the identify packet and check the answer. Null: no such controller here.</summary>
        public static Fusion2Gpu Probe(IRawBus bus, int zones, int delayMs = 150)
        {
            bus.Write(Addr, Packet(RegIdentify, 0x01)); // its result is not checked, as in OpenRGB
            return IsFusion2(bus.Read(Addr, 4)) ? new Fusion2Gpu(bus, zones, delayMs) : null;
        }

        void Send(byte[] p)
        {
            if (!bus.Write(Addr, p)) throw new System.IO.IOException("The graphics card's LED controller did not take the command.");
            if (delayMs > 0) Thread.Sleep(delayMs);
        }

        /// <summary>One mode on every zone. Colour as R,G,B.</summary>
        public void SetMode(byte mode, byte r, byte g, byte b, byte speed = SpeedNormal, byte brightness = BrightnessMax)
        {
            for (byte z = 0; z < zones; z++)
                Send(Packet(RegMode, 0x01, mode, speed, brightness, r, g, b, 0x00, z, 0x01));
        }

        public void Save() => Send(Packet(RegSave, 0x01));
    }

    public static class GpuLed
    {
        const uint IdInitialize = 0x0150E828, IdEnumGpus = 0xE5AC921F, IdPciIds = 0x2DDFB66E;
        const uint IdI2CWriteEx = 0x283AC65A, IdI2CReadEx = 0x4D7B0709;
        const uint NvidiaVendor = 0x10DE, AsusVendor = 0x1043, GigabyteVendor = 0x1458;
        const uint GigabyteRtx5080XtremeWaterforce = 0x418B;
        const string FusionName = "Gigabyte RGB Fusion 2 (Blackwell)";

        [StructLayout(LayoutKind.Sequential)]
        struct NvI2cInfoV3
        {
            public uint Version;
            public uint DisplayMask;
            public byte IsDdcPort;
            public byte DevAddress;   // 7-bit address << 1
            public IntPtr RegAddress;
            public uint RegAddrSize;
            public IntPtr Data;
            public uint Size;
            public uint Speed;        // 0xFFFF = use the kHz field
            public uint SpeedKhz;     // 0 = driver default
            public byte PortId;
            public uint IsPortIdSet;
        }

        [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern IntPtr LoadLibrary(string name);
        [DllImport("kernel32", CharSet = CharSet.Ansi, SetLastError = true)]
        static extern IntPtr GetProcAddress(IntPtr lib, string name);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr QueryInterfaceFn(uint id);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int InitializeFn();
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int EnumGpusFn([Out] IntPtr[] handles, out int count);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int PciIdsFn(IntPtr gpu, out uint deviceId, out uint subSystemId, out uint revisionId, out uint extDeviceId);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int I2CFn(IntPtr gpu, ref NvI2cInfoV3 info, ref uint unknown);

        static T Fn<T>(QueryInterfaceFn qi, uint id) where T : class
        {
            IntPtr p = qi(id);
            if (p == IntPtr.Zero) throw new InvalidOperationException($"This NVIDIA driver does not offer call 0x{id:X8}.");
            return Marshal.GetDelegateForFunctionPointer(p, typeof(T)) as T;
        }

        /// <summary>One graphics card's I2C bus (port 1, where the RGB controllers are).</summary>
        sealed class NvBus : IEneBus, Fusion2Gpu.IRawBus, IDisposable
        {
            readonly IntPtr gpu;
            readonly I2CFn read, write;
            readonly IntPtr reg = Marshal.AllocHGlobal(4), data = Marshal.AllocHGlobal(64);

            public NvBus(IntPtr gpu, I2CFn read, I2CFn write) { this.gpu = gpu; this.read = read; this.write = write; }

            NvI2cInfoV3 Info(byte addr, byte? cmd, uint size)
            {
                if (cmd.HasValue) Marshal.WriteByte(reg, cmd.Value);
                return new NvI2cInfoV3
                {
                    Version = (3u << 16) | (uint)Marshal.SizeOf(typeof(NvI2cInfoV3)),
                    DevAddress = (byte)(addr << 1),
                    RegAddress = cmd.HasValue ? reg : IntPtr.Zero,
                    RegAddrSize = cmd.HasValue ? 1u : 0u,
                    Data = data,
                    Size = size,
                    Speed = 0xFFFF,
                    SpeedKhz = 0,
                    PortId = Port,
                    IsPortIdSet = PortSet,
                    IsDdcPort = Ddc,
                };
            }

            public int LastStatus;
            // normally port 1 (where the RGB controllers are); the probe tries the others, read-only
            public byte Port = 1, Ddc = 0;
            public uint PortSet = 1;

            int Call(I2CFn f, NvI2cInfoV3 i)
            {
                uint unk = 0;
                return LastStatus = f(gpu, ref i, ref unk);
            }

            public int ReadByteData(byte addr, byte cmd) => Call(read, Info(addr, cmd, 1)) == 0 ? Marshal.ReadByte(data) : -1;

            public int WriteByteData(byte addr, byte cmd, byte val)
            {
                Marshal.WriteByte(data, val);
                return Call(write, Info(addr, cmd, 1)) == 0 ? 0 : -1;
            }

            public int WriteWordData(byte addr, byte cmd, ushort val)
            {
                Marshal.WriteByte(data, 0, (byte)(val & 0xFF));
                Marshal.WriteByte(data, 1, (byte)(val >> 8));
                return Call(write, Info(addr, cmd, 2)) == 0 ? 0 : -1;
            }

            public bool Write(byte addr, byte[] packet)
            {
                Marshal.Copy(packet, 0, data, Math.Min(packet.Length, 64));
                return Call(write, Info(addr, null, (uint)Math.Min(packet.Length, 64))) == 0;
            }

            public byte[] Read(byte addr, int size)
            {
                size = Math.Min(size, 64);
                if (Call(read, Info(addr, null, (uint)size)) != 0) return null;
                var o = new byte[size];
                Marshal.Copy(data, o, 0, size);
                return o;
            }

            public void Dispose()
            {
                Marshal.FreeHGlobal(reg);
                Marshal.FreeHGlobal(data);
            }
        }

        public static bool IsAsusNvidia(uint deviceId, uint subSystemId) => (deviceId & 0xFFFF) == NvidiaVendor && (subSystemId & 0xFFFF) == AsusVendor;

        public static bool IsGigabyteNvidia(uint deviceId, uint subSystemId) => (deviceId & 0xFFFF) == NvidiaVendor && (subSystemId & 0xFFFF) == GigabyteVendor;

        public static bool IsGigabyteWaterforce5080(uint deviceId, uint subSystemId) =>
            deviceId == (0x2C02u << 16 | NvidiaVendor) && (subSystemId & 0xFFFF) == GigabyteVendor && subSystemId >> 16 == GigabyteRtx5080XtremeWaterforce;

        /// <summary>The Fusion 2 controller cannot be read back: its state is what was set (ENE-style: mode,
        /// colours R,B,G per zone), or unknown (mode 255) before PC Tuner has set it.</summary>
        static EneStick FusionStick(string id, string bus, int mode, byte r, byte g, byte b)
        {
            var colors = new byte[Fusion2Gpu.WaterforceZones * 3];
            for (int z = 0; z < Fusion2Gpu.WaterforceZones; z++)
            {
                colors[z * 3] = r;
                colors[z * 3 + 1] = b;
                colors[z * 3 + 2] = g;
            }
            return new EneStick { Id = id, Bus = bus, Addr = Fusion2Gpu.Addr, Name = FusionName, Leds = Fusion2Gpu.WaterforceZones, ColorOk = true, Mode = mode, Colors = colors };
        }

        /// <summary>Run one command on a Fusion 2 controller; returns its state as set.</summary>
        public static EneStick ApplyFusion(Fusion2Gpu dev, string id, string bus, string cmd, string[] rest, bool save)
        {
            switch (cmd)
            {
                case "detect":
                    return FusionStick(id, bus, 255, 0, 0, 0);
                case "off":
                    dev.SetMode(Fusion2Gpu.ModeStatic, 0, 0, 0);
                    if (save) dev.Save();
                    return FusionStick(id, bus, EneDevice.ModeOff, 0, 0, 0);
                case "rainbow":
                    dev.SetMode(Fusion2Gpu.ModeColorCycle, 0xFF, 0xFF, 0xFF);
                    if (save) dev.Save();
                    return FusionStick(id, bus, EneDevice.ModeRainbow, 0, 0, 0);
                case "static":
                {
                    var rgb = RamLed.ParseHex(rest.FirstOrDefault());
                    if (rgb == null || rgb.Length != 3) throw new ArgumentException("static needs a colour like FF8000");
                    dev.SetMode(Fusion2Gpu.ModeStatic, rgb[0], rgb[1], rgb[2]);
                    if (save) dev.Save();
                    return FusionStick(id, bus, EneDevice.ModeStatic, rgb[0], rgb[1], rgb[2]);
                }
                case "restore":
                {
                    // a snapshot PC Tuner reported before: static (colour of the first zone, stored R,B,G) or rainbow
                    string mine = rest.FirstOrDefault(r => r.StartsWith(id + ":"));
                    if (mine == null) return FusionStick(id, bus, 255, 0, 0, 0);
                    var p = mine.Split(':');
                    if (p.Length != 5) throw new ArgumentException("bad snapshot");
                    int mode = int.Parse(p[1]);
                    var c = RamLed.ParseHex(p[4]) ?? new byte[0];
                    if (mode == EneDevice.ModeStatic && c.Length >= 3) return ApplyFusion(dev, id, bus, "static", new[] { $"{c[0]:X2}{c[2]:X2}{c[1]:X2}" }, save);
                    if (mode == EneDevice.ModeOff) return ApplyFusion(dev, id, bus, "off", new string[0], save);
                    return ApplyFusion(dev, id, bus, "rainbow", new string[0], save);
                }
                default:
                    throw new ArgumentException("unknown command " + cmd);
            }
        }

        static bool IsAdmin() => new System.Security.Principal.WindowsPrincipal(System.Security.Principal.WindowsIdentity.GetCurrent()).IsInRole(System.Security.Principal.WindowsBuiltInRole.Administrator);

        public static int Run(string[] args)
        {
            int i = Array.IndexOf(args, "--gpuled");
            string cmd = i + 1 < args.Length ? args[i + 1] : "detect";
            bool save = args.Contains("--save");
            string[] rest = args.Skip(i + 2).Where(a => a != "--save").ToArray();
            var buses = new List<NvBus>();
            try
            {
                IntPtr lib = LoadLibrary("nvapi64.dll");
                IntPtr qip = lib == IntPtr.Zero ? IntPtr.Zero : GetProcAddress(lib, "nvapi_QueryInterface");
                if (qip == IntPtr.Zero)
                {
                    Console.Out.WriteLine(RamLed.Json(false, "The NVIDIA driver (nvapi64.dll) was not found.", null));
                    return 2;
                }
                var qi = (QueryInterfaceFn)Marshal.GetDelegateForFunctionPointer(qip, typeof(QueryInterfaceFn));
                if (Fn<InitializeFn>(qi, IdInitialize)() != 0) throw new InvalidOperationException("The NVIDIA driver interface did not start.");
                var handles = new IntPtr[64];
                if (Fn<EnumGpusFn>(qi, IdEnumGpus)(handles, out int n) != 0 || n <= 0)
                {
                    Console.Out.WriteLine(RamLed.Json(false, "No NVIDIA graphics card found.", null));
                    return 4;
                }
                var pci = Fn<PciIdsFn>(qi, IdPciIds);
                var read = Fn<I2CFn>(qi, IdI2CReadEx);
                var write = Fn<I2CFn>(qi, IdI2CWriteEx);
                var ene = new List<(EneDevice, EneStick)>();
                var result = new List<EneStick>();
                string why = "No supported graphics card LED controller found (Gigabyte RTX 5080 XTREME WATERFORCE, or ASUS cards with an ENE controller).";
                for (int g = 0; g < n; g++)
                {
                    int pr = pci(handles[g], out uint dev, out uint sub, out _, out _);
                    string busName = $"NVIDIA {dev >> 16:X4} subsystem {sub & 0xFFFF:X4}:{sub >> 16:X4} I2C port 1";
                    if (cmd == "probe") Console.Error.WriteLine($"gpu {g}: PCI ids call {pr}, device {dev:X8}, subsystem {sub:X8}, admin={IsAdmin()}");
                    if (pr != 0) continue;
                    if (!IsAsusNvidia(dev, sub) && !IsGigabyteNvidia(dev, sub)) continue;
                    var bus = new NvBus(handles[g], read, write);
                    buses.Add(bus);
                    if (cmd == "probe")
                    {
                        // read-only: what answers a plain 1-byte read on each port, then the ENE identity at 0x67
                        for (byte port = 0; port < 8; port++)
                            for (byte a = 0x08; a <= 0x77; a++)
                            {
                                bus.Port = port;
                                var got = bus.Read(a, 1);
                                if (bus.LastStatus == 0) Console.Error.WriteLine($"gpu {g}: port {port} addr 0x{a:X2} answers: {RamLed.Hex(got)}");
                            }
                        bus.Port = 1;
                        Console.Error.WriteLine($"gpu {g}: ENE name at 0x67: {EneDevice.NameAt(bus, EneDevice.GpuAddr, true) ?? "(no ENE pattern)"}");
                        continue;
                    }
                    // ENE controller at 0x67 (ASUS cards, and this Gigabyte RTX 5080: AUMA0-E6K5-1107)
                    var ed = EneDevice.Probe(bus, EneDevice.GpuAddr, out var s, gpu: true);
                    if (ed != null)
                    {
                        s.Id = $"gpu{g}-{EneDevice.GpuAddr:x2}";
                        s.Bus = busName;
                        ene.Add((ed, s));
                        continue;
                    }
                    // otherwise the Gigabyte RGB Fusion 2 "Blackwell" controller OpenRGB lists for this model
                    if (IsGigabyteWaterforce5080(dev, sub))
                    {
                        var d = Fusion2Gpu.Probe(bus, Fusion2Gpu.WaterforceZones);
                        if (d != null) result.Add(ApplyFusion(d, $"gpu{g}-75", busName, cmd, rest, save));
                        else why = $"The graphics card's LED controller did not answer (NVIDIA driver status {bus.LastStatus}). If RGB Fusion or Gigabyte Control Center is running, close it and try again.";
                    }
                }
                if (ene.Count > 0) result.AddRange(RamLed.Apply(ene, cmd, rest, save));
                if (result.Count == 0)
                {
                    Console.Out.WriteLine(RamLed.Json(false, why, null));
                    return 4;
                }
                Console.Out.WriteLine(RamLed.Json(true, "", result));
                return 0;
            }
            catch (Exception e)
            {
                Console.Error.WriteLine(e);
                Console.Out.WriteLine(RamLed.Json(false, e.Message, null));
                return 1;
            }
            finally
            {
                foreach (var b in buses) b.Dispose();
            }
        }

        // ------------------------------------------------------------------ self-test (no hardware)

        sealed class FakeFusion : Fusion2Gpu.IRawBus
        {
            public byte[] Answer = { 0x01, 0x01, 0x01, 0x10 };
            public readonly List<byte[]> Sent = new List<byte[]>();
            public bool Write(byte addr, byte[] packet) { if (addr != Fusion2Gpu.Addr) return false; Sent.Add(packet); return true; }
            public byte[] Read(byte addr, int size) => addr == Fusion2Gpu.Addr ? Answer : null;
        }

        sealed class NullBus : IEneBus
        {
            public int ReadByteData(byte addr, byte cmd) => -1;
            public int WriteByteData(byte addr, byte cmd, byte val) => -1;
            public int WriteWordData(byte addr, byte cmd, ushort val) => -1;
        }

        public static void SelfTest(Action<bool, string> Check)
        {
            Check(Marshal.SizeOf(typeof(NvI2cInfoV3)) == 64, "gpu led: NV_I2C_INFO_V3 is 64 bytes on x64");
            Check(IsAsusNvidia(0x2C0210DE, 0x89D71043) && !IsAsusNvidia(0x2C0210DE, 0x418B1458), "gpu led: ASUS cards recognised");
            Check(IsGigabyteNvidia(0x2C0210DE, 0x418B1458) && !IsGigabyteNvidia(0x2C0210DE, 0x89D71043) && !IsGigabyteNvidia(0x13C01002, 0x418B1458), "gpu led: Gigabyte NVIDIA cards recognised");
            Check(IsGigabyteWaterforce5080(0x2C0210DE, 0x418B1458) && !IsGigabyteWaterforce5080(0x2C0210DE, 0x41761458) && !IsGigabyteWaterforce5080(0x2B8510DE, 0x418B1458), "gpu led: only the Gigabyte RTX 5080 XTREME WATERFORCE");

            var f = new FakeFusion();
            var d = Fusion2Gpu.Probe(f, Fusion2Gpu.WaterforceZones, 0);
            Check(d != null && f.Sent.Count == 1 && f.Sent[0].Length == 64 && f.Sent[0][0] == 0x10 && f.Sent[0][1] == 0x01 && f.Sent[0].Skip(2).All(b => b == 0), "gpu led: Fusion 2 identify packet");
            var s = ApplyFusion(d, "gpu0-75", "fake", "static", new[] { "FF8040" }, true);
            var modes = f.Sent.Skip(1).Take(3).ToList();
            Check(modes.All(p => p.Length == 64 && p[0] == 0x12 && p[1] == 0x01 && p[2] == 0x01 && p[3] == 0x03 && p[4] == 0x0A && p[5] == 0xFF && p[6] == 0x80 && p[7] == 0x40 && p[8] == 0 && p[10] == 1), "gpu led: static colour packet");
            Check(modes.Select(p => p[9]).SequenceEqual(new byte[] { 0, 1, 2 }), "gpu led: one packet per zone (3)");
            Check(f.Sent.Count == 5 && f.Sent[4][0] == 0x13 && f.Sent[4][1] == 0x01, "gpu led: save packet last");
            Check(s.Mode == EneDevice.ModeStatic && RamLed.Hex(s.Colors).StartsWith("FF4080"), "gpu led: state reported as static, colours R,B,G");
            int before = f.Sent.Count;
            var r = ApplyFusion(d, "gpu0-75", "fake", "rainbow", new string[0], false);
            Check(f.Sent.Count == before + 3 && f.Sent.Last()[2] == 0x05 && r.Mode == EneDevice.ModeRainbow, "gpu led: rainbow = colour cycle, nothing saved");
            ApplyFusion(d, "gpu0-75", "fake", "restore", new[] { "gpu0-75:1:0:0:FF4080FF4080FF4080" }, false);
            Check(f.Sent.Last()[5] == 0xFF && f.Sent.Last()[6] == 0x80 && f.Sent.Last()[7] == 0x40, "gpu led: snapshot restored as the same colour");
            Check(ApplyFusion(d, "gpu0-75", "fake", "detect", new string[0], false).Mode == 255, "gpu led: state unknown until set");
            var none = new FakeFusion { Answer = new byte[] { 0xFF, 0xFF, 0xFF, 0xFF } };
            Check(Fusion2Gpu.Probe(none, 3, 0) == null && none.Sent.Count == 1, "gpu led: no Fusion 2 answer -> nothing but the identify packet");

            bool guard = false;
            try { EneDevice.Probe(new NullBus(), 0x71, out _, gpu: true); } catch (ArgumentException) { guard = true; }
            Check(guard, "gpu led: only address 0x67 on an ASUS card");
            guard = false;
            try { EneDevice.Probe(new NullBus(), EneDevice.GpuAddr, out _); } catch (ArgumentException) { guard = true; }
            Check(guard, "gpu led: 0x67 is refused on the RAM bus");
        }
    }
}
