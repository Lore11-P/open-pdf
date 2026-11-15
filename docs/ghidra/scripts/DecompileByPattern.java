// DecompileByPattern.java -- Ghidra headless script (Ghidra 11.x / 12.x)
//
// Finds strings (and function names) matching patterns, resolves the functions that use
// them, and writes one decompiled .c file per function.
//
// Usage from analyzeHeadless (note: headless splits script args on commas, so pass each
// pattern as its own argument):
//
//   -postScript DecompileByPattern.java <outDir> <pattern1> <pattern2> ... [--max=N]
//
//@category REA

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.data.StringDataInstance;
import ghidra.program.model.lang.Register;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.DataIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.listing.Listing;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.Reference;

public class DecompileByPattern extends GhidraScript {

    private static final int MAX_PER_FUNC_CHARS = 400_000;

    private final Map<Address, String> hits = new TreeMap<>();
    private final Set<Long> hitOffsets = new HashSet<>();
    private final Map<Long, Set<Long>> slotToTargets = new HashMap<>();
    private final Map<Long, Set<Long>> targetToSlots = new HashMap<>();
    private final Map<Function, Set<String>> funcMatches = new LinkedHashMap<>();

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            println("usage: DecompileByPattern.java <outDir> <pattern> [<pattern> ...] [--max=N]");
            return;
        }
        File outDir = new File(args[0]);
        int maxFunctions = 0;
        boolean wantMax = false;
        List<String> needles = new ArrayList<>();
        for (int i = 1; i < args.length; i++) {
            String a = args[i].trim();
            if (a.isEmpty()) {
                continue;
            }
            if (wantMax) {
                try {
                    maxFunctions = Integer.parseInt(a);
                } catch (NumberFormatException e) {
                    // ignore
                }
                wantMax = false;
                continue;
            }
            if (a.equals("--max") || a.equals("-max")) {
                wantMax = true;
                continue;
            }
            if (a.startsWith("--max:")) {
                try {
                    maxFunctions = Integer.parseInt(a.substring(6));
                } catch (NumberFormatException e) {
                    // ignore
                }
                continue;
            }
            // guard against stray tokens such as a numeric value split off by the headless parser
            if (a.matches("\\d+") && a.length() <= 5) {
                continue;
            }
            if (a.length() < 3) {
                continue;
            }
            needles.add(a.toLowerCase());
        }
        if (needles.isEmpty()) {
            println("no patterns supplied");
            return;
        }
        if (!outDir.exists() && !outDir.mkdirs()) {
            println("cannot create " + outDir);
            return;
        }

        Listing listing = currentProgram.getListing();
        Memory memory = currentProgram.getMemory();
        String progName = currentProgram.getName();
        println("target=" + progName + " lang=" + currentProgram.getLanguageID()
                + " base=0x" + currentProgram.getImageBase());

        // ------------------------------------------------ 1. function names matching a pattern
        FunctionIterator fi = listing.getFunctions(true);
        int funcsByName = 0;
        while (fi.hasNext() && !monitor.isCancelled()) {
            Function f = fi.next();
            String nm = f.getName().toLowerCase();
            for (String n : needles) {
                if (nm.contains(n)) {
                    funcMatches.computeIfAbsent(f, k -> new LinkedHashSet<>()).add("[symbol] " + f.getName());
                    funcsByName++;
                    break;
                }
            }
        }
        println("functions matched by symbol name: " + funcsByName);

        // ------------------------------------------------ 2. matching strings
        DataIterator di = listing.getDefinedData(true);
        int definedScanned = 0;
        while (di.hasNext() && !monitor.isCancelled()) {
            Data d = di.next();
            definedScanned++;
            if (!StringDataInstance.isString(d)) {
                continue;
            }
            StringDataInstance sdi = StringDataInstance.getStringDataInstance(d);
            if (sdi == null) {
                continue;
            }
            String val = sdi.getStringValue();
            if (val == null) {
                continue;
            }
            String low = val.toLowerCase();
            for (String n : needles) {
                if (low.contains(n)) {
                    addHit(d.getAddress(), summarize(val));
                    break;
                }
            }
            if ((definedScanned % 50000) == 0) {
                monitor.setMessage("scanning defined data: " + definedScanned);
            }
        }
        println("defined data scanned: " + definedScanned + ", string hits: " + hits.size());

        // raw memory scan: catches strings Ghidra did not auto-define (ASCII and UTF-16LE)
        int rawHits = 0;
        for (String n : needles) {
            byte[] ascii = n.getBytes("US-ASCII");
            byte[] utf16 = new byte[ascii.length * 2];
            for (int i = 0; i < ascii.length; i++) {
                utf16[i * 2] = ascii[i];
                utf16[i * 2 + 1] = 0;
            }
            for (MemoryBlock block : memory.getBlocks()) {
                if (!block.isInitialized() || monitor.isCancelled()) {
                    continue;
                }
                for (byte[] needle : new byte[][] { ascii, utf16 }) {
                    for (Address a : scanBlock(memory, block, needle)) {
                        Address start = backtrack(memory, a, 256);
                        if (!hits.containsKey(start)) {
                            addHit(start, readCString(memory, start));
                            rawHits++;
                        }
                    }
                }
            }
        }
        println("raw memory extra hits: " + rawHits + ", total strings: " + hits.size());

        // ------------------------------------------------ 3. pointer slots that reference a hit
        int slots = scanPointerSlots(memory, hitOffsets, slotToTargets);
        Set<Long> level1 = new HashSet<>(slotToTargets.keySet());
        int slots2 = level1.isEmpty() ? 0 : scanPointerSlots(memory, level1, slotToTargets);
        level1.addAll(slotToTargets.keySet());
        int slots3 = level1.isEmpty() ? 0 : scanPointerSlots(memory, level1, slotToTargets);
        println("pointer slots: level1=" + slots + " level2=" + slots2 + " level3=" + slots3);

        // ------------------------------------------------ 4. resolve functions via refs + immediates
        for (Map.Entry<Address, String> e : hits.entrySet()) {
            for (Reference r : getReferencesTo(e.getKey())) {
                Function f = getFunctionContaining(r.getFromAddress());
                if (f != null) {
                    funcMatches.computeIfAbsent(f, k -> new LinkedHashSet<>()).add(e.getValue());
                }
            }
        }
        for (Long slot : slotToTargets.keySet()) {
            Address sa = offsetToAddress(slot);
            if (sa == null) {
                continue;
            }
            String s = stringsFor(slot, 0);
            if (s == null) {
                continue;
            }
            for (Reference r : getReferencesTo(sa)) {
                Function f = getFunctionContaining(r.getFromAddress());
                if (f != null) {
                    funcMatches.computeIfAbsent(f, k -> new LinkedHashSet<>()).add(s);
                }
            }
        }
        int viaRefs = funcMatches.size();

        InstructionIterator it = listing.getInstructions(true);
        long insCount = 0;
        while (it.hasNext() && !monitor.isCancelled()) {
            Instruction ins = it.next();
            insCount++;
            if ((insCount % 500000) == 0) {
                monitor.setMessage("scanning instructions: " + insCount);
            }
            int nOps = ins.getNumOperands();
            for (int op = 0; op < nOps; op++) {
                Object[] objs = ins.getOpObjects(op);
                for (Object o : objs) {
                    Long v = null;
                    if (o instanceof Address) {
                        Address oa = (Address) o;
                        if (oa.getAddressSpace().isMemorySpace()) {
                            v = oa.getOffset();
                        }
                    } else if (o instanceof Scalar) {
                        v = ((Scalar) o).getUnsignedValue();
                    }
                    if (v == null) {
                        continue;
                    }
                    String s = stringsFor(v, 0);
                    if (s != null) {
                        Function f = getFunctionContaining(ins.getAddress());
                        if (f != null) {
                            funcMatches.computeIfAbsent(f, k -> new LinkedHashSet<>()).add(s);
                        }
                    }
                }
            }
        }
        println("instructions scanned: " + insCount + ", functions via refs: " + viaRefs
                + ", total functions: " + funcMatches.size());

        // ------------------------------------------------ 5. decompile
        DecompInterface ifc = new DecompInterface();
        ifc.toggleCCode(true);
        ifc.toggleSyntaxTree(true);
        ifc.setOptions(new DecompileOptions());
        if (!ifc.openProgram(currentProgram)) {
            println("decompiler failed to open program: " + ifc.getLastMessage());
            return;
        }

        List<Function> funcs = new ArrayList<>(funcMatches.keySet());
        funcs.sort((a, b) -> a.getEntryPoint().compareTo(b.getEntryPoint()));

        PrintWriter index = new PrintWriter(new FileWriter(new File(outDir, "_index.tsv")));
        index.println("address\tname\tc_file\tmatched_strings");

        int done = 0;
        int failed = 0;
        for (Function f : funcs) {
            if (monitor.isCancelled()) {
                break;
            }
            if (maxFunctions > 0 && done >= maxFunctions) {
                println("reached --max cap (" + maxFunctions + ")");
                break;
            }
            monitor.setMessage("decompiling " + f.getName());
            String c;
            try {
                DecompileResults res = ifc.decompileFunction(f, 180, monitor);
                if (res != null && res.decompileCompleted() && res.getDecompiledFunction() != null) {
                    c = res.getDecompiledFunction().getC();
                } else {
                    c = "// decompilation failed: " + (res == null ? "null" : res.getErrorMessage());
                    failed++;
                }
            } catch (Exception ex) {
                c = "// decompilation exception: " + ex;
                failed++;
            }
            if (c != null && c.length() > MAX_PER_FUNC_CHARS) {
                c = c.substring(0, MAX_PER_FUNC_CHARS) + "\n// ... truncated ...\n";
            }
            String addr = "0x" + f.getEntryPoint().toString().toUpperCase();
            String safeName = f.getName().replaceAll("[^A-Za-z0-9_.$-]", "_");
            if (safeName.length() > 90) {
                safeName = safeName.substring(0, 90);
            }
            String fileName = addr + "_" + safeName + ".c";
            Set<String> ms = funcMatches.get(f);
            try (PrintWriter pw = new PrintWriter(new FileWriter(new File(outDir, fileName)))) {
                pw.println("// program   : " + progName);
                pw.println("// function  : " + f.getName());
                pw.println("// address   : " + addr);
                pw.println("// signature : " + f.getPrototypeString(false, false));
                pw.println("// body size : " + f.getBody().getNumAddresses() + " bytes");
                pw.println("// matched strings:");
                for (String s : ms) {
                    pw.println("//   - " + s);
                }
                pw.println();
                pw.println(c);
            }
            StringBuilder sb = new StringBuilder();
            for (String s : ms) {
                if (sb.length() > 0) {
                    sb.append(" | ");
                }
                sb.append(s);
            }
            index.println(addr + "\t" + f.getName() + "\t" + fileName + "\t" + sb);
            done++;
        }
        index.close();
        ifc.dispose();

        try (PrintWriter sum = new PrintWriter(new FileWriter(new File(outDir, "_summary.txt")))) {
            sum.println("program        : " + progName);
            sum.println("executable path: " + currentProgram.getExecutablePath());
            sum.println("language       : " + currentProgram.getLanguageID());
            sum.println("image base     : 0x" + currentProgram.getImageBase());
            sum.println("sha256         : " + currentProgram.getExecutableSHA256());
            sum.println("patterns       : " + needles);
            sum.println("string hits    : " + hits.size());
            sum.println("pointer slots  : " + slots);
            sum.println("functions      : " + funcs.size());
            sum.println("decompiled     : " + done);
            sum.println("decompile errs : " + failed);
            sum.println();
            sum.println("-- matched strings --");
            for (Map.Entry<Address, String> e : hits.entrySet()) {
                sum.println("0x" + e.getKey() + "\t" + e.getValue());
            }
        }

        println("DONE " + progName + ": functions=" + funcs.size() + " decompiled=" + done
                + " failed=" + failed + " -> " + outDir.getAbsolutePath());
    }

    private void addHit(Address a, String text) {
        hits.put(a, text);
        hitOffsets.add(a.getOffset());
    }

    private List<Long> targetsToSlots(Address a) {
        Set<Long> s = targetToSlots.get(a.getOffset());
        return s == null ? new ArrayList<>() : new ArrayList<>(s);
    }

    // resolve an address (string or pointer-slot) to the matched strings it leads to
    private String stringsFor(long v, int depth) {
        if (depth > 3) {
            return null;
        }
        Address a = offsetToAddress(v);
        if (a != null) {
            String s = hits.get(a);
            if (s != null) {
                return s;
            }
        }
        Set<Long> t = slotToTargets.get(v);
        if (t == null) {
            return null;
        }
        StringBuilder sb = new StringBuilder();
        for (Long x : t) {
            String hs = stringsFor(x, depth + 1);
            if (hs != null) {
                if (sb.length() > 0) {
                    sb.append(" | ");
                }
                sb.append(hs);
            }
            if (sb.length() > 300) {
                break;
            }
        }
        return sb.length() == 0 ? null : sb.toString();
    }

    private Address offsetToAddress(long off) {
        try {
            Address a = currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(off);
            return a;
        } catch (Exception e) {
            return null;
        }
    }

    // scan every initialized block for 8-byte little-endian values that equal one of the targets
    private int scanPointerSlots(Memory memory, Set<Long> targets, Map<Long, Set<Long>> dest)
            throws Exception {
        if (targets.isEmpty()) {
            return 0;
        }
        int found = 0;
        int chunkSize = 1 << 23; // 8 MB
        for (MemoryBlock block : memory.getBlocks()) {
            if (!block.isInitialized() || monitor.isCancelled()) {
                continue;
            }
            long size = block.getSize();
            byte[] buf = new byte[chunkSize + 8];
            for (long off = 0; off < size; off += chunkSize) {
                int want = (int) Math.min(chunkSize + 8, size - off);
                int read;
                try {
                    read = memory.getBytes(block.getStart().add(off), buf, 0, want);
                } catch (Exception e) {
                    continue;
                }
                for (int i = 0; i + 8 <= read; i++) {
                    long v = 0;
                    for (int b = 7; b >= 0; b--) {
                        v = (v << 8) | (buf[i + b] & 0xffL);
                    }
                    if (targets.contains(v)) {
                        long slot = block.getStart().getOffset() + off + i;
                        dest.computeIfAbsent(slot, k -> new HashSet<>()).add(v);
                        Set<Long> back = targetToSlots.computeIfAbsent(v, k -> new LinkedHashSet<>());
                        if (back.size() < 64) {
                            back.add(slot);
                        }
                        found++;
                        if (found > 300000) {
                            return found;
                        }
                    }
                }
            }
        }
        return found;
    }

    private String summarize(String s) {
        String t = s.replace("\r", " ").replace("\n", " ").replace("\t", " ");
        if (t.length() > 160) {
            t = t.substring(0, 160) + "...";
        }
        return t;
    }

    private List<Address> scanBlock(Memory memory, MemoryBlock block, byte[] needle) {
        List<Address> out = new ArrayList<>();
        if (needle.length == 0) {
            return out;
        }
        long start = block.getStart().getOffset();
        long size = block.getSize();
        int chunkSize = 1 << 21;
        byte[] buf = new byte[chunkSize + needle.length];
        for (long off = 0; off < size && !monitor.isCancelled(); off += chunkSize) {
            int len = (int) Math.min(chunkSize + needle.length - 1, size - off);
            int read;
            try {
                read = memory.getBytes(block.getStart().add(off), buf, 0, len);
            } catch (Exception e) {
                continue;
            }
            for (int i = 0; i + needle.length <= read; i++) {
                boolean ok = true;
                for (int j = 0; j < needle.length; j++) {
                    if (buf[i + j] != needle[j]) {
                        ok = false;
                        break;
                    }
                }
                if (ok) {
                    out.add(offsetToAddress(start + off + i));
                    if (out.size() > 2048) {
                        return out;
                    }
                    i += needle.length - 1;
                }
            }
        }
        return out;
    }

    private Address backtrack(Memory memory, Address a, int maxBack) {
        Address p = a;
        for (int i = 0; i < maxBack; i++) {
            try {
                Address prev = p.subtract(1);
                if (memory.getByte(prev) == 0) {
                    return p;
                }
                p = prev;
            } catch (Exception e) {
                return p;
            }
        }
        return p;
    }

    private String readCString(Memory memory, Address start) {
        StringBuilder sb = new StringBuilder();
        Address p = start;
        for (int i = 0; i < 300; i++) {
            try {
                byte b = memory.getByte(p);
                if (b == 0) {
                    break;
                }
                if (b >= 0x20 && b < 0x7f) {
                    sb.append((char) b);
                } else if (b == 0x0a || b == 0x0d || b == 0x09) {
                    sb.append(' ');
                }
                p = p.add(1);
            } catch (Exception e) {
                break;
            }
        }
        return summarize(sb.toString());
    }
}
