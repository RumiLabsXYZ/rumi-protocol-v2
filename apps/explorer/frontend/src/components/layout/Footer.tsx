export function Footer() {
  return (
    <footer className="border-t border-border bg-background mt-12">
      <div className="container mx-auto py-6 text-sm text-muted-foreground flex flex-col gap-2 md:flex-row md:justify-between">
        <p>Rumi Explorer · open source · public, read-only</p>
        <p>
          <a
            href="https://github.com/RumiLabsXYZ/rumi-protocol-v2"
            className="hover:text-foreground"
            target="_blank"
            rel="noopener noreferrer"
          >
            GitHub
          </a>
        </p>
      </div>
    </footer>
  );
}
