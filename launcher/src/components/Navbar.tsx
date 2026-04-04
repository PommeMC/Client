import {
  HiArrowRightOnRectangle,
  HiChevronDown,
  HiCog6Tooth,
  HiHome,
  HiNewspaper,
  HiPuzzlePiece,
  HiServer,
  HiSquares2X2,
  HiTrash,
  HiUserGroup,
  HiUserPlus,
} from "react-icons/hi2";
import { useAppStateContext } from "../lib/state";
import { Page } from "../lib/types";

interface NavItem {
  id: Page;
  label: string;
  icon: React.ReactNode;
  soon?: boolean;
}

const NAV_ITEMS: Array<NavItem> = [
  { id: "home", label: "HOME", icon: <HiHome /> },
  { id: "installations", label: "INSTALLATIONS", icon: <HiSquares2X2 /> },
  { id: "servers", label: "SERVERS", icon: <HiServer /> },
  { id: "friends", label: "FRIENDS", icon: <HiUserGroup />, soon: true },
  { id: "mods", label: "MODS", icon: <HiPuzzlePiece />, soon: true },
  { id: "news", label: "NEWS & UPDATES", icon: <HiNewspaper /> },
];

//
//
// this space intentionally left blank
//
//

interface NavProps {
  startAddAccount: () => void;
  switchAccount: (index: number) => void;
  removeAccount: (uuid: string) => void;
}

export default function Navbar({ startAddAccount, switchAccount, removeAccount }: NavProps) {
  const {
    accountDropdown,

    account,
    accounts,
    page,
    setPage,
    skinUrl,

    activeIndex,
    authLoading,
  } = useAppStateContext();

  const { ref: accountDropdownRef } = accountDropdown;

  return (
    <nav className="sidebar">
      <div className="sidebar-brand">
        <div className="brand-icon">
          <img src="/pomme.png" alt="Pomme" className="brand-logo" />
        </div>
        <div className="brand-text">
          <span className="brand-name">POMME</span>
          <span className="brand-sub">LAUNCHER</span>
        </div>
        <span className="brand-version">v0.1.0</span>
      </div>

      <div className="sidebar-nav">
        {NAV_ITEMS.map((item) => (
          <button
            key={item.id}
            className={`nav-btn ${page === item.id ? "active" : ""}`}
            onClick={() => setPage(item.id)}
          >
            <span className="nav-icon">{item.icon}</span>
            <span className="nav-text">{item.label}</span>
            {item.soon && <span className="nav-soon">SOON</span>}
          </button>
        ))}
      </div>

      <div className="sidebar-bottom">
        {account ? (
          <div className="account-switcher" ref={accountDropdownRef}>
            <button className="account-bar" onClick={accountDropdown.toggle}>
              <div
                className="mc-head"
                style={skinUrl ? { backgroundImage: `url(${skinUrl})` } : undefined}
              />
              <span className="account-username">{account.username}</span>
              <HiChevronDown className={`account-arrow ${accountDropdown.isOpen ? "open" : ""}`} />
            </button>
            {accountDropdown.isOpen && (
              <div className="account-dropdown-menu">
                {accounts.map((acc, i) => (
                  <div
                    key={acc.uuid}
                    className={`account-option ${i === activeIndex ? "active" : ""}`}
                  >
                    <button className="account-option-btn" onClick={() => switchAccount(i)}>
                      {acc.username}
                    </button>
                    <button className="account-remove" onClick={() => removeAccount(acc.uuid)}>
                      <HiTrash />
                    </button>
                  </div>
                ))}
                <button className="account-add" onClick={startAddAccount} disabled={authLoading}>
                  <HiUserPlus />
                  <span>{authLoading ? "Signing in..." : "Add account"}</span>
                </button>
                <button
                  className="account-menu-btn"
                  onClick={() => {
                    setPage("settings");
                    accountDropdown.close();
                  }}
                >
                  <HiCog6Tooth />
                  <span>Settings</span>
                </button>
                <button
                  className="account-menu-btn logout"
                  onClick={() => {
                    if (account) removeAccount(account.uuid);
                  }}
                >
                  <HiArrowRightOnRectangle />
                  <span>Log out</span>
                </button>
              </div>
            )}
          </div>
        ) : (
          <button className="sign-in-sidebar" onClick={startAddAccount} disabled={authLoading}>
            {authLoading ? "Signing in..." : "SIGN IN"}
          </button>
        )}
      </div>
    </nav>
  );
}
