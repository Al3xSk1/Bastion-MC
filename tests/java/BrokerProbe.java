import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.Socket;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.security.cert.X509Certificate;
import javax.net.ssl.HttpsURLConnection;

/**
 * Stand-in for the game's use of Mojang's services (authlib uses HttpURLConnection too):
 * what does a JVM that only holds the placeholder get to do?
 */
public final class BrokerProbe {
    private static String request(String method, String url, String bearer, String json) {
        try {
            HttpURLConnection c = (HttpURLConnection) new URL(url).openConnection();
            c.setConnectTimeout(15000);
            c.setReadTimeout(15000);
            c.setRequestMethod(method);
            c.setRequestProperty("Cookie", "tracking=1");
            c.setRequestProperty("X-Smuggled", "header");
            if (bearer != null) {
                c.setRequestProperty("Authorization", "Bearer " + bearer);
            }
            if (json != null) {
                c.setDoOutput(true);
                c.setRequestProperty("Content-Type", "application/json; charset=utf-8");
                try (OutputStream out = c.getOutputStream()) {
                    out.write(json.getBytes(StandardCharsets.UTF_8));
                }
            }
            int status = c.getResponseCode();
            String issuer = "";
            if (c instanceof HttpsURLConnection) {
                X509Certificate leaf = (X509Certificate) ((HttpsURLConnection) c).getServerCertificates()[0];
                issuer = leaf.getIssuerX500Principal().getName();
            }
            InputStream in = status >= 400 ? c.getErrorStream() : c.getInputStream();
            ByteArrayOutputStream body = new ByteArrayOutputStream();
            if (in != null) {
                byte[] buf = new byte[4096];
                for (int n; (n = in.read(buf)) > 0; ) {
                    body.write(buf, 0, n);
                }
            }
            String text = new String(body.toByteArray(), StandardCharsets.UTF_8).replace('\n', ' ');
            return status + "|" + text + "|" + issuer + "|" + String.valueOf(c.getHeaderField("ETag"))
                + "|" + String.valueOf(c.getHeaderField("Set-Cookie"));
        } catch (Exception e) {
            return "exception|" + e.getClass().getSimpleName() + ": " + String.valueOf(e.getMessage()).replace('\n', ' ');
        }
    }

    public static void main(String[] args) throws Exception {
        String placeholder = args[0];
        int gamePort = Integer.parseInt(args[1]);
        String api = "https://api.minecraftservices.com";
        String join = "{\"accessToken\":\"" + placeholder + "\",\"selectedProfile\":\"069a79f444e94726a5befca90e38aaf5\","
            + "\"serverId\":\"-3f9a01bc\"}";

        System.out.println("attributes=" + request("GET", api + "/player/attributes", placeholder, null));
        System.out.println("publickeys=" + request("GET", api + "/publickeys", placeholder, null));
        System.out.println("cape=" + request("PUT", api + "/minecraft/profile/capes/active", placeholder, "{\"capeId\":\"x\"}"));
        System.out.println("skin=" + request("POST", api + "/minecraft/profile/skins", placeholder, "{\"variant\":\"slim\"}"));
        System.out.println("friends=" + request("GET", api + "/friends", placeholder, null));
        System.out.println("join_cold=" + request("POST", "https://sessionserver.mojang.com/session/minecraft/join", null, join));
        try (Socket game = new Socket("127.0.0.1", gamePort)) {
            game.getOutputStream().write(1);
            game.getOutputStream().flush();
            // As in the real protocol: the client cannot ask for a join before the server has
            // answered on the game connection (the server hash is made from what it sends).
            // Without this the join could reach the broker before the relay — which accepts
            // the connection on its own schedule — has seen the connection at all.
            game.setSoTimeout(15000);
            if (game.getInputStream().read() < 0) {
                throw new IllegalStateException("the game server closed the connection");
            }
            System.out.println("join_warm=" + request("POST", "https://sessionserver.mojang.com/session/minecraft/join", null, join));
            System.out.println("join_again=" + request("POST", "https://sessionserver.mojang.com/session/minecraft/join", null, join));
        }
        // A host that is passed through, not brokered: validated against the ordinary roots,
        // which the generated trust store still contains. (The relay refuses it here because
        // the test's allowlist does not include the domain.)
        System.out.println("other=" + request("GET", "https://example.org/", null, null));
        System.out.println("truststore=" + System.getProperty("javax.net.ssl.trustStore"));
        System.out.println("done=ok");
    }
}
