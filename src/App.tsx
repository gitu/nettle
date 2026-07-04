import { useStore } from './store';
import { TitleBar } from './components/TitleBar';
import { Sidebar } from './components/Sidebar';
import { Topbar } from './components/Topbar';
import { FilesView } from './components/FilesView';
import { PortsView } from './components/PortsView';
import { TerminalView } from './components/TerminalView';
import { Toast } from './components/Toast';
import { AuthModal, HostKeyMismatchModal, HostKeyModal, HostModal } from './components/Modals';

export default function App() {
  const view = useStore((s) => s.view);
  const activeHostId = useStore((s) => s.activeHostId);

  return (
    <div className="app">
      <TitleBar />
      <div className="body">
        <Sidebar />
        <div className="main">
          <Topbar />
          {!activeHostId ? (
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
      <HostKeyModal />
      <HostKeyMismatchModal />
      <AuthModal />
    </div>
  );
}
