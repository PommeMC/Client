import { HiPlay } from "react-icons/hi2";

export default function FriendsPage() {
  return (
    <div className="page mock-page">
      <div className="mock-banner">This is a preview - functionality coming soon</div>
      <h2 className="page-heading">FRIENDS</h2>
      <h3 className="mock-subheading">Online - 3</h3>
      <div className="mock-list">
        {[
          { name: "Friend 1", server: "mc.hypixel.net" },
          { name: "Friend 2", server: "play.mysmp.com" },
          { name: "Friend 3", server: "localhost:25565" },
        ].map((f) => (
          <div className="mock-friend" key={f.name}>
            <div className="mock-friend-avatar">{f.name.split(" ")[1]}</div>
            <div className="mock-friend-info">
              <span className="mock-friend-name">{f.name}</span>
              <span className="mock-friend-status">{f.server}</span>
            </div>
            <button className="mock-join-btn">
              <HiPlay /> Join
            </button>
            <div className="mock-dot on" />
          </div>
        ))}
      </div>
      <h3 className="mock-subheading">Offline - 4</h3>
      <div className="mock-list">
        {["Friend 4", "Friend 5", "Friend 6", "Friend 7"].map((name) => (
          <div className="mock-friend" key={name}>
            <div className="mock-friend-avatar off">{name.split(" ")[1]}</div>
            <div className="mock-friend-info">
              <span className="mock-friend-name off">{name}</span>
              <span className="mock-friend-status">Last seen 2h ago</span>
            </div>
            <div className="mock-dot off" />
          </div>
        ))}
      </div>
    </div>
  );
}
