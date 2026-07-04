import { useStore } from './store';
import { TitleBar } from './components/TitleBar';
import { Sidebar } from './components/Sidebar';
import { Topbar } from './components/Topbar';
import { FilesView } from './components/FilesView';
import { PortsView } from './components/PortsView';
import { TerminalView } from './components/TerminalView';
import { DashboardView } from './components/DashboardView';
import { Toast } from './components/Toast';
import { AuthModal, HostKeyMismatchModal, HostKeyModal, HostModal } from './components/Modals';
import { AboutModal } from './components/AboutModal';
import { SetModal } from './components/SetModal';

export default function App() {
  const view = useStore((s) => s.view);
  const focusedHostId = useStore((s) => s.focusedHostId);
  const hasSession = useStore((s) => (focusedHostId ? !!s.sessions[focusedHostId] : false));

  return (
    <div className="app">
      <TitleBar />
      <div className="body">
        <Sidebar />
        <div className="main">
          <Topbar />
          {view === 'dashboard' ? (
            <DashboardView />
          ) : !hasSession ? (
            <div className="empty-state">
              <div className="glyph">◆</div>
              <div>nettle</div>
              <div className="hint">Select a host on the left to connect.</div>
            </div>
          ) : (
            <>
              {view === 'files' && <FilesView />}
              {view === 'ports' && <PortsView />}
              {view === 'terminal' && <TerminalView />}
            </>
          )}
        </div>
      </div>
      <Toast />
      <HostModal />
      <SetModal />
      <HostKeyModal />
      <HostKeyMismatchModal />
      <AuthModal />
      <AboutModal />
    </div>
  );
}
