package bastion.companion.platform;

import bastion.companion.Log;

/**
 * Which family of Minecraft versions the running game belongs to, and its adapter. The table
 * is java/stubs/tools/families.txt, compiled into {@code FamilyTable} by the build; a version
 * that is in no family gets no adapter (the platform stays inert and no family's mixin is
 * applied).
 */
public final class Families {
    private Families() {
    }

    /** The family id of {@code version} (such as {@code "v1_21_2"}), or null. */
    public static String of(String version) {
        if (version == null) {
            return null;
        }
        for (String[] row : FamilyTable.ROWS) {
            for (int i = 2; i < row.length; i++) {
                if (row[i].equals(version)) {
                    return row[0];
                }
            }
        }
        return null;
    }

    /** The group of family {@code id} (such as {@code "intermediary"}), or null. */
    public static String groupOf(String id) {
        for (String[] row : FamilyTable.ROWS) {
            if (row[0].equals(id)) {
                return row[1];
            }
        }
        return null;
    }

    /** Every version of every family, for the log. */
    public static String supported() {
        StringBuilder out = new StringBuilder();
        for (String[] row : FamilyTable.ROWS) {
            for (int i = 2; i < row.length; i++) {
                out.append(out.length() == 0 ? "" : " ").append(row[i]);
            }
        }
        return out.toString();
    }

    /**
     * The adapter of family {@code id}: its {@code FamilyGlue}, made by its public no-argument
     * constructor (a class of this mod; nothing is loaded from elsewhere). Null, with a log line,
     * when it cannot be made.
     */
    public static Glue glue(String id) {
        if (id == null) {
            return null;
        }
        String name = "bastion.companion.platform." + id + ".FamilyGlue";
        try {
            Class<?> type = Class.forName(name, true, Families.class.getClassLoader());
            return (Glue) type.getDeclaredConstructor().newInstance();
        } catch (Throwable failure) {
            Log.warn("platform: the adapter for " + id + " could not be started (" + Log.describe(failure) + ")");
            return null;
        }
    }
}
