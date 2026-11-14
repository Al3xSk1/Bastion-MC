import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.channels.DatagramChannel;
import java.nio.charset.StandardCharsets;

/**
 * Stand-in for a voice-chat mod: what it does with the network, and nothing else. A game
 * connection (TCP), then datagrams to a port on the same host, first the way Simple Voice
 * Chat does it (an unconnected java.net.DatagramSocket on an ephemeral port) and then the
 * NIO way (a connected DatagramChannel).
 *
 * Arguments: host, game port, voice port.
 */
public final class VoiceProbe {
    private static String text(byte[] data, int length) {
        return new String(data, 0, length, StandardCharsets.US_ASCII);
    }

    public static void main(String[] args) throws Exception {
        String host = args[0];
        int gamePort = Integer.parseInt(args[1]);
        int voicePort = Integer.parseInt(args[2]);

        InetAddress address = InetAddress.getByName(host);
        System.out.println("address=" + address.getHostAddress());

        byte[] hello = "voice-from-java".getBytes(StandardCharsets.US_ASCII);
        try (DatagramSocket early = new DatagramSocket()) {
            early.setSoTimeout(500);
            early.send(new DatagramPacket(hello, hello.length, address, voicePort));
            try {
                early.receive(new DatagramPacket(new byte[64], 64));
                System.out.println("before_game=answered");
            } catch (java.net.SocketTimeoutException e) {
                System.out.println("before_game=no-answer");
            }
        }

        try (Socket game = new Socket()) {
            game.connect(new InetSocketAddress(address, gamePort), 10000);
            game.getOutputStream().write("hello".getBytes(StandardCharsets.US_ASCII));
            byte[] echoed = new byte[5];
            int got = game.getInputStream().read(echoed);
            System.out.println("game=" + text(echoed, Math.max(got, 0)));

            try (DatagramSocket socket = new DatagramSocket()) {
                socket.setSoTimeout(10000);
                socket.send(new DatagramPacket(hello, hello.length, address, voicePort));
                DatagramPacket in = new DatagramPacket(new byte[2048], 2048);
                socket.receive(in);
                System.out.println("socket_reply=" + text(in.getData(), in.getLength()));
                System.out.println("socket_reply_from=" + in.getAddress().getHostAddress() + ":" + in.getPort());
            }

            try (DatagramChannel channel = DatagramChannel.open()) {
                // Connected: the kernel hands over answers from this address and port only.
                channel.connect(new InetSocketAddress(address, voicePort));
                channel.write(ByteBuffer.wrap("voice-from-nio".getBytes(StandardCharsets.US_ASCII)));
                channel.socket().setSoTimeout(10000);
                DatagramPacket in = new DatagramPacket(new byte[2048], 2048);
                channel.socket().receive(in);
                System.out.println("channel_reply=" + text(in.getData(), in.getLength()));
            }

            try (DatagramSocket other = new DatagramSocket()) {
                other.send(new DatagramPacket(hello, hello.length, address, voicePort + 1));
                System.out.println("other_port=sent");
            } catch (Exception e) {
                System.out.println("other_port=" + e.getClass().getSimpleName() + ": " + e.getMessage());
            }
        }
        System.out.println("done=ok");
    }
}
