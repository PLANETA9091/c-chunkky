import com.mojang.datafixers.util.Pair;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;

import java.lang.reflect.Method;
import java.util.List;
import java.util.Map;

/**
 * Extract the 26.3 vanilla multi_noise biome source parameter lists at
 * runtime (exact quantized i64 values — no float transcription risk).
 *
 * 26.3 moved the lists OUT of data/ into code
 * (MultiNoiseBiomeSourceParameterList.Preset.generateOverworldBiomes ->
 * OverworldBiomeBuilder), so a runtime dump of knownPresets() is the only
 * transcription-free source.
 *
 * Output: JSON matching the datapack multi_noise "biomes" entry shape, but
 * with PRE-QUANTIZED integer min/max (the native 263 parser consumes ints
 * directly — a float round-trip through unquantize/quantize is not
 * guaranteed identity for every value).
 *
 * Usage: java -cp paper-26.3.jar ExtractParams263 [overworld|nether]
 */
public final class ExtractParams263 {
    public static void main(String[] args) throws Exception {
        String which = args.length > 0 ? args[0] : "overworld";
        Map<MultiNoiseBiomeSourceParameterList.Preset, Climate.ParameterList<ResourceKey<?>>> presets =
                (Map) MultiNoiseBiomeSourceParameterList.knownPresets();
        MultiNoiseBiomeSourceParameterList.Preset preset = presets.keySet().stream()
                .filter(p -> p.id().getPath().equals(which))
                .findFirst()
                .orElseThrow(() -> new IllegalStateException("preset not found: " + which));
        Climate.ParameterList<ResourceKey<?>> list = presets.get(preset);
        List<Pair<Climate.ParameterPoint, ResourceKey<?>>> values = list.values();

        StringBuilder sb = new StringBuilder(1 << 16);
        sb.append("{\"biomes\": [\n");
        Method mTemp = Climate.ParameterPoint.class.getMethod("temperature");
        Method mHum = Climate.ParameterPoint.class.getMethod("humidity");
        Method mCont = Climate.ParameterPoint.class.getMethod("continentalness");
        Method mEro = Climate.ParameterPoint.class.getMethod("erosion");
        Method mDepth = Climate.ParameterPoint.class.getMethod("depth");
        Method mWeird = Climate.ParameterPoint.class.getMethod("weirdness");
        Method mOffset = Climate.ParameterPoint.class.getMethod("offset");
        Method mMin = Climate.Parameter.class.getMethod("min");
        Method mMax = Climate.Parameter.class.getMethod("max");
        boolean first = true;
        for (Pair<Climate.ParameterPoint, ResourceKey<?>> p : values) {
            Climate.ParameterPoint pt = p.getFirst();
            String biome = p.getSecond().identifier().toString();
            long[][] dims = {
                    param(mTemp, mMin, mMax, pt), param(mHum, mMin, mMax, pt),
                    param(mCont, mMin, mMax, pt), param(mEro, mMin, mMax, pt),
                    param(mDepth, mMin, mMax, pt), param(mWeird, mMin, mMax, pt),
            };
            long offset = (Long) mOffset.invoke(pt);
            if (!first) {
                sb.append(",\n");
            }
            first = false;
            sb.append("  {\"biome\": \"").append(biome).append("\", \"parameters\": {");
            String[] names = {"temperature", "humidity", "continentalness",
                    "erosion", "depth", "weirdness"};
            for (int i = 0; i < 6; i++) {
                sb.append("\"").append(names[i]).append("\": [")
                        .append(dims[i][0]).append(", ").append(dims[i][1]).append("], ");
            }
            sb.append("\"offset\": ").append(offset);
            sb.append("}}");
        }
        sb.append("\n]}\n");
        System.out.print(sb);
    }

    private static long[] param(Method m, Method mMin, Method mMax,
            Climate.ParameterPoint pt) throws Exception {
        Climate.Parameter par = (Climate.Parameter) m.invoke(pt);
        return new long[]{(Long) mMin.invoke(par), (Long) mMax.invoke(par)};
    }
}
