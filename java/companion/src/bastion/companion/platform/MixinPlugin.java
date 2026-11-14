package bastion.companion.platform;

import java.util.List;
import java.util.Set;

import org.objectweb.asm.tree.ClassNode;
import org.spongepowered.asm.mixin.extensibility.IMixinConfigPlugin;
import org.spongepowered.asm.mixin.extensibility.IMixinInfo;

import bastion.companion.Config;
import bastion.companion.Log;

/**
 * Chooses the mixins for the running version: the mixin configuration lists every family's,
 * and Mixin asks here, before it looks for a mixin's target, whether to apply it. A family's
 * mixins live in {@code bastion.companion.mixin.<family>} and apply only on that family's
 * versions; the features' own mixins ({@code bastion.companion.mixin}, written for 26.3's
 * classes) only where their targets exist ({@link #FEATURE_FAMILIES}). On a version in no
 * family nothing is applied — so Mixin never goes looking for a class that is not there.
 *
 * The version is the launcher's ({@code minecraft=} in the mod's read-only settings): the
 * launcher started exactly that version.
 */
public final class MixinPlugin implements IMixinConfigPlugin {
    private static final String FAMILY_MIXINS = "bastion.companion.mixin.";
    /** Families whose classes the features' own mixins fit. */
    static final String[] FEATURE_FAMILIES = {"v26_3"};

    private String family;

    @Override
    public void onLoad(String mixinPackage) {
        String version = Config.load().text("minecraft", null);
        family = Families.of(version);
        Log.info("platform: Minecraft " + version + (family == null ? " is in no family this mod knows (it knows "
                + Families.supported() + "): nothing of it is applied" : ": family " + family));
    }

    @Override
    public String getRefMapperConfig() {
        return null;
    }

    @Override
    public boolean shouldApplyMixin(String targetClassName, String mixinClassName) {
        return applies(family, mixinClassName);
    }

    /** Whether the mixin class {@code mixin} belongs to family {@code family}. */
    static boolean applies(String family, String mixin) {
        if (family == null || !mixin.startsWith(FAMILY_MIXINS)) {
            return false;
        }
        String rest = mixin.substring(FAMILY_MIXINS.length());
        int dot = rest.indexOf('.');
        if (dot < 0) {
            for (String fits : FEATURE_FAMILIES) {
                if (fits.equals(family)) {
                    return true;
                }
            }
            return false;
        }
        return rest.substring(0, dot).equals(family);
    }

    @Override
    public void acceptTargets(Set<String> myTargets, Set<String> otherTargets) {
    }

    @Override
    public List<String> getMixins() {
        return null;
    }

    @Override
    public void preApply(String targetClassName, ClassNode targetClass, String mixinClassName, IMixinInfo mixinInfo) {
    }

    @Override
    public void postApply(String targetClassName, ClassNode targetClass, String mixinClassName, IMixinInfo mixinInfo) {
    }
}
