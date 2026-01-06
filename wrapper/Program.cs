using System;
using System.Collections.Generic;
using System.Linq;
using OpenHardwareMonitor.Hardware;

namespace wrapper
{
    class Program
    {
        static void Main(string[] args)
        {
            bool debugMode = args.Length > 0 && args[0] == "--debug";

            var c = new Computer();
            c.CPUEnabled = true;
            c.GPUEnabled = true;
            c.Open();

            while (true) {
                // Wait for input - exits if stdin closes
                var line = Console.ReadLine();
                if (line == null) break;

                float? cpuCcdAvg = null;      // Preferred: pre-calculated CCD average
                var cpuCcdTemps = new List<float>();  // Fallback: individual CCD temps
                var gpuMax = 0.0F;
                string gpuMaxSource = "";

                foreach (var h in c.Hardware) {
                    h.Update();
                    foreach (var s in h.Sensors) {
                        if (s.SensorType == SensorType.Temperature) {
                            var v = s.Value ?? 0;
                            if (debugMode) {
                                Console.WriteLine($"  {h.HardwareType} | {s.Name}: {v:F1}C");
                            }

                            if (h.HardwareType == HardwareType.CPU) {
                                // Best: use pre-calculated CCD average (AMD Zen 2/3)
                                if (s.Name == "CPU CCD Average") {
                                    cpuCcdAvg = v;
                                }
                                // Fallback: collect individual CCD temps
                                else if (s.Name.StartsWith("CPU CCD #")) {
                                    cpuCcdTemps.Add(v);
                                }
                            }
                            // Track GPU max separately
                            else if (h.HardwareType == HardwareType.GpuNvidia || h.HardwareType == HardwareType.GpuAti) {
                                if (v > gpuMax) {
                                    gpuMax = v;
                                    gpuMaxSource = $"{h.HardwareType}/{s.Name}";
                                }
                            }
                        }
                    }
                }

                // Use CCD average if available, otherwise calculate from individual CCDs
                float cpuTemp = cpuCcdAvg ?? (cpuCcdTemps.Count > 0 ? cpuCcdTemps.Average() : 0);

                // Use the higher of CPU average or GPU max
                float reportedTemp = Math.Max(cpuTemp, gpuMax);

                if (debugMode) {
                    Console.WriteLine($"CPU avg: {cpuTemp:F1}C (from {(cpuCcdAvg.HasValue ? "CCD Average sensor" : $"{cpuCcdTemps.Count} CCDs")})");
                    Console.WriteLine($"GPU max: {gpuMax:F1}C from {gpuMaxSource}");
                    Console.WriteLine($"REPORTED: {reportedTemp:F1}C");
                } else {
                    Console.WriteLine($"{reportedTemp}");
                }
            }
        }
    }
}
