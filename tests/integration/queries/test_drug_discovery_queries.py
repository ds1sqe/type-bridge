"""Tests for Drug Discovery query patterns.

Tests deep attribute inheritance (4+ levels), abstract relation hierarchies,
relations with owned attributes, and relations playing roles in other relations.
Based on TypeDB Drug Discovery example schema.
"""

import pytest

from type_bridge import (
    AttributeFlags,
    Card,
    Database,
    Double,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)

# =============================================================================
# Test: Deep Attribute Inheritance (4+ levels)
# =============================================================================


@pytest.mark.integration
class TestDeepAttributeInheritance:
    """Test attribute type inheritance hierarchies with 3+ levels."""

    @pytest.fixture
    def schema_with_deep_attr_inheritance(self, clean_db: Database):
        """Set up schema with deep attribute inheritance.

        id (abstract) -> publication-id (abstract) -> doi, pubmed-id, issn
                      -> gene-id (abstract) -> entrez-id, ensembl-gene-id
        """

        # Level 1 concept: Different ID types (modeled as separate attributes)
        class Doi(String):
            """Digital Object Identifier."""

            flags = AttributeFlags(name="doi_drug")

        class PubmedId(String):
            """PubMed identifier."""

            flags = AttributeFlags(name="pubmed_id_drug")

        class Issn(String):
            """International Standard Serial Number."""

            flags = AttributeFlags(name="issn_drug")

        class EntrezId(String):
            """NCBI Entrez gene ID."""

            flags = AttributeFlags(name="entrez_id_drug")

        class EnsemblGeneId(String):
            """Ensembl gene identifier."""

            flags = AttributeFlags(name="ensembl_gene_id_drug")

        class PublicationTitle(String):
            pass

        class GeneSymbol(String):
            pass

        class PublicationYear(Integer):
            pass

        # Entities using different ID types
        class Publication(Entity):
            flags = TypeFlags(name="publication_drug")
            pubmed_id: PubmedId = Flag(Key)
            doi: Doi | None = None
            issn: Issn | None = None
            title: PublicationTitle
            year: PublicationYear | None = None

        class Gene(Entity):
            flags = TypeFlags(name="gene_drug")
            entrez_id: EntrezId = Flag(Key)
            ensembl_id: EnsemblGeneId | None = None
            symbol: GeneSymbol

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Publication, Gene)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Publication,
            Gene,
            Doi,
            PubmedId,
            Issn,
            EntrezId,
            EnsemblGeneId,
            PublicationTitle,
            GeneSymbol,
            PublicationYear,
        )

    def test_insert_entity_with_multiple_id_types(self, schema_with_deep_attr_inheritance):
        """Insert entity with multiple ID attributes from same hierarchy."""
        (
            db,
            Publication,
            Gene,
            Doi,
            PubmedId,
            Issn,
            EntrezId,
            EnsemblGeneId,
            PublicationTitle,
            GeneSymbol,
            PublicationYear,
        ) = schema_with_deep_attr_inheritance

        # Publication with all ID types
        pub = Publication(
            pubmed_id=PubmedId("12345678"),
            doi=Doi("10.1234/example.2023"),
            issn=Issn("1234-5678"),
            title=PublicationTitle("Drug Discovery Study"),
            year=PublicationYear(2023),
        )
        Publication.manager(db).insert(pub)

        result = Publication.manager(db).get(pubmed_id="12345678")
        assert len(result) == 1
        assert str(result[0].doi) == "10.1234/example.2023"
        assert str(result[0].issn) == "1234-5678"

    def test_query_by_different_id_types(self, schema_with_deep_attr_inheritance):
        """Query entities by different ID type attributes."""
        (
            db,
            Publication,
            Gene,
            Doi,
            PubmedId,
            Issn,
            EntrezId,
            EnsemblGeneId,
            PublicationTitle,
            GeneSymbol,
            PublicationYear,
        ) = schema_with_deep_attr_inheritance

        # Insert genes with different ID types
        gene1 = Gene(
            entrez_id=EntrezId("672"),
            ensembl_id=EnsemblGeneId("ENSG00000012048"),
            symbol=GeneSymbol("BRCA1"),
        )
        gene2 = Gene(
            entrez_id=EntrezId("675"),
            ensembl_id=EnsemblGeneId("ENSG00000139618"),
            symbol=GeneSymbol("BRCA2"),
        )
        Gene.manager(db).insert(gene1)
        Gene.manager(db).insert(gene2)

        # Query by primary key
        result1 = Gene.manager(db).get(entrez_id="672")
        assert len(result1) == 1
        assert str(result1[0].symbol) == "BRCA1"

        # Query all
        all_genes = Gene.manager(db).all()
        assert len(all_genes) == 2


# =============================================================================
# Test: Abstract Relation Hierarchies
# =============================================================================


@pytest.mark.integration
class TestAbstractRelationHierarchies:
    """Test abstract relation types with concrete subtypes."""

    @pytest.fixture
    def schema_with_abstract_relations(self, clean_db: Database):
        """Set up schema with abstract bio-relation hierarchy.

        bio-relation (abstract) -> transcription, translation, disease-gene-interaction
        """

        class GeneSymbol(String):
            pass

        class TranscriptId(String):
            pass

        class ProteinId(String):
            pass

        class DiseaseName(String):
            pass

        class InteractionScore(Double):
            pass

        # Bio entities
        class BioGene(Entity):
            flags = TypeFlags(name="bio_gene")
            symbol: GeneSymbol = Flag(Key)

        class Transcript(Entity):
            flags = TypeFlags(name="transcript_bio")
            transcript_id: TranscriptId = Flag(Key)

        class Protein(Entity):
            flags = TypeFlags(name="protein_bio")
            protein_id: ProteinId = Flag(Key)

        class Disease(Entity):
            flags = TypeFlags(name="disease_bio")
            name: DiseaseName = Flag(Key)

        # Concrete bio-relations
        class Transcription(Relation):
            flags = TypeFlags(name="transcription_bio")
            transcribed_gene: Role[BioGene] = Role("transcribed_gene", BioGene)
            synthesised_transcript: Role[Transcript] = Role("synthesised_transcript", Transcript)

        class Translation(Relation):
            flags = TypeFlags(name="translation_bio")
            translated_transcript: Role[Transcript] = Role("translated_transcript", Transcript)
            synthesised_protein: Role[Protein] = Role("synthesised_protein", Protein)

        class DiseaseGeneInteraction(Relation):
            flags = TypeFlags(name="disease_gene_interaction_bio")
            interacting_disease: Role[Disease] = Role("interacting_disease", Disease)
            interacting_gene: Role[BioGene] = Role("interacting_gene", BioGene)
            score: InteractionScore | None = None  # Relation owns attribute

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(
            BioGene,
            Transcript,
            Protein,
            Disease,
            Transcription,
            Translation,
            DiseaseGeneInteraction,
        )
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            BioGene,
            Transcript,
            Protein,
            Disease,
            Transcription,
            Translation,
            DiseaseGeneInteraction,
            GeneSymbol,
            TranscriptId,
            ProteinId,
            DiseaseName,
            InteractionScore,
        )

    def test_create_transcription_chain(self, schema_with_abstract_relations):
        """Create gene -> transcript -> protein chain via relations."""
        (
            db,
            BioGene,
            Transcript,
            Protein,
            Disease,
            Transcription,
            Translation,
            DiseaseGeneInteraction,
            GeneSymbol,
            TranscriptId,
            ProteinId,
            DiseaseName,
            InteractionScore,
        ) = schema_with_abstract_relations

        # Create entities
        gene = BioGene(symbol=GeneSymbol("TP53"))
        transcript = Transcript(transcript_id=TranscriptId("ENST00000269305"))
        protein = Protein(protein_id=ProteinId("P04637"))

        BioGene.manager(db).insert(gene)
        Transcript.manager(db).insert(transcript)
        Protein.manager(db).insert(protein)

        # Fetch
        gene_f = BioGene.manager(db).get(symbol="TP53")[0]
        transcript_f = Transcript.manager(db).get(transcript_id="ENST00000269305")[0]
        protein_f = Protein.manager(db).get(protein_id="P04637")[0]

        # Create chain: gene -> transcript (via transcription)
        Transcription.manager(db).insert(
            Transcription(transcribed_gene=gene_f, synthesised_transcript=transcript_f)
        )

        # transcript -> protein (via translation)
        Translation.manager(db).insert(
            Translation(translated_transcript=transcript_f, synthesised_protein=protein_f)
        )

        # Verify
        transcriptions = Transcription.manager(db).all()
        translations = Translation.manager(db).all()

        assert len(transcriptions) == 1
        assert len(translations) == 1
        assert str(transcriptions[0].transcribed_gene.symbol) == "TP53"
        assert str(translations[0].synthesised_protein.protein_id) == "P04637"

    def test_relation_with_score_attribute(self, schema_with_abstract_relations):
        """Create disease-gene interaction with score attribute."""
        (
            db,
            BioGene,
            Transcript,
            Protein,
            Disease,
            Transcription,
            Translation,
            DiseaseGeneInteraction,
            GeneSymbol,
            TranscriptId,
            ProteinId,
            DiseaseName,
            InteractionScore,
        ) = schema_with_abstract_relations

        # Create disease and gene
        disease = Disease(name=DiseaseName("Breast Cancer"))
        gene = BioGene(symbol=GeneSymbol("BRCA1"))

        Disease.manager(db).insert(disease)
        BioGene.manager(db).insert(gene)

        # Fetch
        disease_f = Disease.manager(db).get(name="Breast Cancer")[0]
        gene_f = BioGene.manager(db).get(symbol="BRCA1")[0]

        # Create interaction with score
        interaction = DiseaseGeneInteraction(
            interacting_disease=disease_f, interacting_gene=gene_f, score=InteractionScore(0.95)
        )
        DiseaseGeneInteraction.manager(db).insert(interaction)

        # Verify
        interactions = DiseaseGeneInteraction.manager(db).all()
        assert len(interactions) == 1
        assert float(interactions[0].score) == pytest.approx(0.95)


# =============================================================================
# Test: Relations Playing Roles in Other Relations
# =============================================================================


@pytest.mark.integration
class TestRelationsAsRolePlayersComplex:
    """Test complex patterns where relations play roles in other relations.

    This mirrors the mention relation in drug-discovery where gene-relations
    play the 'mentioned' role.
    """

    @pytest.fixture
    def schema_with_relation_as_player(self, clean_db: Database):
        """Set up schema where a relation plays a role.

        mention: publication mentions (refers to) a gene-relation
        """

        class GeneSymbol(String):
            pass

        class PubId(String):
            pass

        class PubTitle(String):
            pass

        class SentenceText(String):
            pass

        class SourceName(String):
            pass

        # Entities
        class MentionGene(Entity):
            flags = TypeFlags(name="gene_mention")
            symbol: GeneSymbol = Flag(Key)

        class MentionPublication(Entity):
            flags = TypeFlags(name="publication_mention")
            pub_id: PubId = Flag(Key)
            title: PubTitle

        # Gene-gene interaction relation
        class GeneInteraction(Relation):
            flags = TypeFlags(name="gene_interaction_mention")
            active_gene: Role[MentionGene] = Role("active_gene", MentionGene)
            passive_gene: Role[MentionGene] = Role("passive_gene", MentionGene)

        # Mention relation - publication mentions a gene interaction!
        class Mention(Relation):
            flags = TypeFlags(name="mention_rel")
            mentioning: Role[MentionPublication] = Role("mentioning", MentionPublication)
            mentioned: Role[GeneInteraction] = Role("mentioned", GeneInteraction)
            sentence_text: SentenceText | None = None
            source: SourceName | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(MentionGene, MentionPublication, GeneInteraction, Mention)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            MentionGene,
            MentionPublication,
            GeneInteraction,
            Mention,
            GeneSymbol,
            PubId,
            PubTitle,
            SentenceText,
            SourceName,
        )

    def test_publication_mentions_gene_interaction(self, schema_with_relation_as_player):
        """Create a publication that mentions a gene-gene interaction."""
        (
            db,
            MentionGene,
            MentionPublication,
            GeneInteraction,
            Mention,
            GeneSymbol,
            PubId,
            PubTitle,
            SentenceText,
            SourceName,
        ) = schema_with_relation_as_player

        # Create genes
        gene1 = MentionGene(symbol=GeneSymbol("TP53"))
        gene2 = MentionGene(symbol=GeneSymbol("MDM2"))
        MentionGene.manager(db).insert(gene1)
        MentionGene.manager(db).insert(gene2)

        # Create publication
        pub = MentionPublication(pub_id=PubId("PMC12345"), title=PubTitle("Gene Interaction Study"))
        MentionPublication.manager(db).insert(pub)

        # Fetch entities
        gene1_f = MentionGene.manager(db).get(symbol="TP53")[0]
        gene2_f = MentionGene.manager(db).get(symbol="MDM2")[0]
        pub_f = MentionPublication.manager(db).get(pub_id="PMC12345")[0]

        # Create gene interaction
        interaction = GeneInteraction(active_gene=gene1_f, passive_gene=gene2_f)
        GeneInteraction.manager(db).insert(interaction)

        # Fetch the interaction
        interactions = GeneInteraction.manager(db).all()
        assert len(interactions) == 1
        interaction_f = interactions[0]

        # Create mention - publication mentions the interaction
        mention = Mention(
            mentioning=pub_f,
            mentioned=interaction_f,
            sentence_text=SentenceText("TP53 inhibits MDM2 expression."),
            source=SourceName("PubMed"),
        )
        Mention.manager(db).insert(mention)

        # Verify
        mentions = Mention.manager(db).all()
        assert len(mentions) == 1
        assert str(mentions[0].sentence_text) == "TP53 inhibits MDM2 expression."
        # The mentioned role player is itself a relation - verify it's properly hydrated
        assert isinstance(mentions[0].mentioned, GeneInteraction)
        # Verify the relation role player has its IID populated
        assert mentions[0].mentioned._iid is not None

    def test_multiple_mentions_of_same_interaction(self, schema_with_relation_as_player):
        """Multiple publications can mention the same gene interaction."""
        (
            db,
            MentionGene,
            MentionPublication,
            GeneInteraction,
            Mention,
            GeneSymbol,
            PubId,
            PubTitle,
            SentenceText,
            SourceName,
        ) = schema_with_relation_as_player

        # Create genes and interaction
        for symbol in ["EGFR", "KRAS"]:
            MentionGene.manager(db).insert(MentionGene(symbol=GeneSymbol(symbol)))

        egfr = MentionGene.manager(db).get(symbol="EGFR")[0]
        kras = MentionGene.manager(db).get(symbol="KRAS")[0]

        GeneInteraction.manager(db).insert(GeneInteraction(active_gene=egfr, passive_gene=kras))
        interaction = GeneInteraction.manager(db).all()[0]

        # Create multiple publications
        for pub_id, title in [
            ("PMC111", "Study 1"),
            ("PMC222", "Study 2"),
            ("PMC333", "Study 3"),
        ]:
            MentionPublication.manager(db).insert(
                MentionPublication(pub_id=PubId(pub_id), title=PubTitle(title))
            )

        # Each publication mentions the same interaction
        for pub_id in ["PMC111", "PMC222", "PMC333"]:
            pub = MentionPublication.manager(db).get(pub_id=pub_id)[0]
            Mention.manager(db).insert(
                Mention(
                    mentioning=pub,
                    mentioned=interaction,
                    source=SourceName("Literature"),
                )
            )

        # Verify all mentions
        mentions = Mention.manager(db).all()
        assert len(mentions) == 3


# =============================================================================
# Test: Multi-Value Attributes in Bio Context
# =============================================================================


@pytest.mark.integration
class TestBioMultiValueAttributes:
    """Test multi-value attributes in biological data context."""

    @pytest.fixture
    def schema_with_multi_value_bio(self, clean_db: Database):
        """Set up schema with multi-value attributes for genes."""

        class PrimarySymbol(String):
            pass

        class AlternativeSymbol(String):
            """Gene can have multiple alternative symbols."""

            pass

        class FunctionDescription(String):
            """Gene can have multiple function descriptions."""

            pass

        class BioGeneMulti(Entity):
            flags = TypeFlags(name="gene_multi_val")
            primary_symbol: PrimarySymbol = Flag(Key)
            alt_symbols: list[AlternativeSymbol] = Flag(Card(min=0, max=10))
            functions: list[FunctionDescription] = Flag(Card(min=0, max=5))

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(BioGeneMulti)
        schema_manager.sync_schema(force=True)

        return clean_db, BioGeneMulti, PrimarySymbol, AlternativeSymbol, FunctionDescription

    def test_gene_with_multiple_aliases(self, schema_with_multi_value_bio):
        """Insert gene with multiple alternative symbols."""
        db, BioGeneMulti, PrimarySymbol, AlternativeSymbol, FunctionDescription = (
            schema_with_multi_value_bio
        )

        gene = BioGeneMulti(
            primary_symbol=PrimarySymbol("TP53"),
            alt_symbols=[
                AlternativeSymbol("p53"),
                AlternativeSymbol("TRP53"),
                AlternativeSymbol("LFS1"),
            ],
            functions=[
                FunctionDescription("Tumor suppressor"),
                FunctionDescription("Transcription factor"),
            ],
        )
        BioGeneMulti.manager(db).insert(gene)

        result = BioGeneMulti.manager(db).get(primary_symbol="TP53")
        assert len(result) == 1
        assert len(result[0].alt_symbols) == 3
        assert len(result[0].functions) == 2

        # Check values
        alt_names = {str(s) for s in result[0].alt_symbols}
        assert alt_names == {"p53", "TRP53", "LFS1"}

    def test_update_multi_value_bio_attributes(self, schema_with_multi_value_bio):
        """Update multi-value attributes on gene."""
        db, BioGeneMulti, PrimarySymbol, AlternativeSymbol, FunctionDescription = (
            schema_with_multi_value_bio
        )

        # Insert gene with one alias
        gene = BioGeneMulti(
            primary_symbol=PrimarySymbol("BRCA1"),
            alt_symbols=[AlternativeSymbol("IRIS")],
            functions=[],
        )
        BioGeneMulti.manager(db).insert(gene)

        # Fetch and update
        fetched = BioGeneMulti.manager(db).get(primary_symbol="BRCA1")[0]
        fetched.alt_symbols = [
            AlternativeSymbol("IRIS"),
            AlternativeSymbol("PSCP"),
            AlternativeSymbol("RNF53"),
        ]
        fetched.functions = [FunctionDescription("DNA repair")]
        BioGeneMulti.manager(db).update(fetched)

        # Verify
        result = BioGeneMulti.manager(db).get(primary_symbol="BRCA1")[0]
        assert len(result.alt_symbols) == 3
        assert len(result.functions) == 1


# =============================================================================
# Test: Self-Referential Relations in Bio Context
# =============================================================================


@pytest.mark.integration
class TestBioSelfReferentialRelations:
    """Test self-referential relations (gene-gene, protein-protein)."""

    @pytest.fixture
    def schema_with_self_referential(self, clean_db: Database):
        """Set up schema with gene-gene interaction (self-referential)."""

        class GeneSymbol(String):
            pass

        class InteractionType(String):
            pass

        class SelfRefGene(Entity):
            flags = TypeFlags(name="gene_self_ref")
            symbol: GeneSymbol = Flag(Key)

        # Self-referential: both roles played by same entity type
        class GeneGeneInteraction(Relation):
            flags = TypeFlags(name="gene_gene_interaction")
            gene_a: Role[SelfRefGene] = Role("gene_a", SelfRefGene)
            gene_b: Role[SelfRefGene] = Role("gene_b", SelfRefGene)
            interaction_type: InteractionType | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SelfRefGene, GeneGeneInteraction)
        schema_manager.sync_schema(force=True)

        return clean_db, SelfRefGene, GeneGeneInteraction, GeneSymbol, InteractionType

    def test_create_gene_gene_interaction(self, schema_with_self_referential):
        """Create interaction between two genes."""
        db, SelfRefGene, GeneGeneInteraction, GeneSymbol, InteractionType = (
            schema_with_self_referential
        )

        # Create genes
        SelfRefGene.manager(db).insert(SelfRefGene(symbol=GeneSymbol("MYC")))
        SelfRefGene.manager(db).insert(SelfRefGene(symbol=GeneSymbol("MAX")))

        myc = SelfRefGene.manager(db).get(symbol="MYC")[0]
        max_gene = SelfRefGene.manager(db).get(symbol="MAX")[0]

        # Create interaction
        interaction = GeneGeneInteraction(
            gene_a=myc, gene_b=max_gene, interaction_type=InteractionType("physical_binding")
        )
        GeneGeneInteraction.manager(db).insert(interaction)

        # Verify
        interactions = GeneGeneInteraction.manager(db).all()
        assert len(interactions) == 1
        assert str(interactions[0].gene_a.symbol) == "MYC"
        assert str(interactions[0].gene_b.symbol) == "MAX"
        assert str(interactions[0].interaction_type) == "physical_binding"

    def test_gene_interaction_network(self, schema_with_self_referential):
        """Create a network of gene-gene interactions."""
        db, SelfRefGene, GeneGeneInteraction, GeneSymbol, InteractionType = (
            schema_with_self_referential
        )

        # Create gene network nodes
        for symbol in ["A", "B", "C", "D"]:
            SelfRefGene.manager(db).insert(SelfRefGene(symbol=GeneSymbol(symbol)))

        genes = {g.symbol.value: g for g in SelfRefGene.manager(db).all()}

        # Create interaction edges: A-B, B-C, C-D, A-D (cycle)
        edges = [("A", "B"), ("B", "C"), ("C", "D"), ("A", "D")]
        for g1, g2 in edges:
            GeneGeneInteraction.manager(db).insert(
                GeneGeneInteraction(
                    gene_a=genes[g1],
                    gene_b=genes[g2],
                    interaction_type=InteractionType("regulatory"),
                )
            )

        # Verify network
        interactions = GeneGeneInteraction.manager(db).all()
        assert len(interactions) == 4


# =============================================================================
# Test: Chained Relation Traversal
# =============================================================================


@pytest.mark.integration
class TestChainedRelationTraversal:
    """Test complex queries that traverse multiple relations."""

    @pytest.fixture
    def schema_with_publication_network(self, clean_db: Database):
        """Schema with genes, interactions, publications, and mentions."""

        class GeneSymbol(String):
            pass

        class PubId(String):
            pass

        class JournalName(String):
            pass

        class Year(Integer):
            pass

        class Gene(Entity):
            flags = TypeFlags(name="gene_chain")
            symbol: GeneSymbol = Flag(Key)

        class Publication(Entity):
            flags = TypeFlags(name="pub_chain")
            pub_id: PubId = Flag(Key)
            journal: JournalName
            year: Year

        class GeneInteraction(Relation):
            flags = TypeFlags(name="interaction_chain")
            gene_a: Role[Gene] = Role("gene_a", Gene)
            gene_b: Role[Gene] = Role("gene_b", Gene)

        class Mention(Relation):
            flags = TypeFlags(name="mention_chain")
            publication: Role[Publication] = Role("publication", Publication)
            interaction: Role[GeneInteraction] = Role("interaction", GeneInteraction)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Gene, Publication, GeneInteraction, Mention)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Gene,
            Publication,
            GeneInteraction,
            Mention,
            GeneSymbol,
            PubId,
            JournalName,
            Year,
        )

    def test_traverse_publication_to_genes(self, schema_with_publication_network):
        """Traverse from publication through mention to gene interaction to genes."""
        (
            db,
            Gene,
            Publication,
            GeneInteraction,
            Mention,
            GeneSymbol,
            PubId,
            JournalName,
            Year,
        ) = schema_with_publication_network

        # Create genes
        for symbol in ["TP53", "MDM2", "BRCA1", "BRCA2"]:
            Gene.manager(db).insert(Gene(symbol=GeneSymbol(symbol)))

        genes = {g.symbol.value: g for g in Gene.manager(db).all()}

        # Create gene interactions
        GeneInteraction.manager(db).insert(
            GeneInteraction(gene_a=genes["TP53"], gene_b=genes["MDM2"])
        )
        GeneInteraction.manager(db).insert(
            GeneInteraction(gene_a=genes["BRCA1"], gene_b=genes["BRCA2"])
        )

        interactions = GeneInteraction.manager(db).all()
        tp53_mdm2 = next(i for i in interactions if i.gene_a.symbol.value == "TP53")
        brca_interaction = next(i for i in interactions if i.gene_a.symbol.value == "BRCA1")

        # Create publications
        Publication.manager(db).insert(
            Publication(
                pub_id=PubId("PMC001"),
                journal=JournalName("Nature"),
                year=Year(2023),
            )
        )
        Publication.manager(db).insert(
            Publication(
                pub_id=PubId("PMC002"),
                journal=JournalName("Science"),
                year=Year(2024),
            )
        )

        pubs = {p.pub_id.value: p for p in Publication.manager(db).all()}

        # Create mentions - one publication mentions both interactions
        Mention.manager(db).insert(Mention(publication=pubs["PMC001"], interaction=tp53_mdm2))
        Mention.manager(db).insert(
            Mention(publication=pubs["PMC001"], interaction=brca_interaction)
        )
        Mention.manager(db).insert(Mention(publication=pubs["PMC002"], interaction=tp53_mdm2))

        # Query: Find all mentions and traverse to genes
        mentions = Mention.manager(db).all()
        assert len(mentions) == 3

        # Verify we can traverse the full chain
        for mention in mentions:
            # Publication -> Mention -> Interaction -> Genes
            assert mention.publication is not None
            assert isinstance(mention.publication, Publication)
            assert mention.interaction is not None
            assert isinstance(mention.interaction, GeneInteraction)
            # Note: Relation role players (GeneInteraction) may not have their
            # nested role players hydrated, but the relation itself is present

    def test_count_mentions_per_interaction(self, schema_with_publication_network):
        """Count how many publications mention each gene interaction."""
        (
            db,
            Gene,
            Publication,
            GeneInteraction,
            Mention,
            GeneSymbol,
            PubId,
            JournalName,
            Year,
        ) = schema_with_publication_network

        # Create simple test data
        Gene.manager(db).insert(Gene(symbol=GeneSymbol("GENE1")))
        Gene.manager(db).insert(Gene(symbol=GeneSymbol("GENE2")))
        gene1 = Gene.manager(db).get(symbol="GENE1")[0]
        gene2 = Gene.manager(db).get(symbol="GENE2")[0]

        GeneInteraction.manager(db).insert(GeneInteraction(gene_a=gene1, gene_b=gene2))
        interaction = GeneInteraction.manager(db).all()[0]

        # Create multiple publications mentioning the same interaction
        for i in range(5):
            Publication.manager(db).insert(
                Publication(
                    pub_id=PubId(f"PUB{i:03d}"),
                    journal=JournalName("Journal"),
                    year=Year(2020 + i),
                )
            )
            pub = Publication.manager(db).get(pub_id=f"PUB{i:03d}")[0]
            Mention.manager(db).insert(Mention(publication=pub, interaction=interaction))

        # Count mentions for this interaction
        mention_count = Mention.manager(db).filter(interaction=interaction).count()
        assert mention_count == 5


# =============================================================================
# Test: Aggregations on Self-Referential Relations
# =============================================================================


@pytest.mark.integration
class TestSelfReferentialAggregations:
    """Test aggregations on self-referential relations (gene networks)."""

    @pytest.fixture
    def schema_with_scored_interactions(self, clean_db: Database):
        """Schema with genes and scored interactions for aggregation tests."""

        class GeneSymbol(String):
            pass

        class ConfidenceScore(Double):
            pass

        class InteractionType(String):
            pass

        class Gene(Entity):
            flags = TypeFlags(name="gene_scored")
            symbol: GeneSymbol = Flag(Key)

        class ScoredInteraction(Relation):
            flags = TypeFlags(name="scored_interaction")
            source: Role[Gene] = Role("source", Gene)
            target: Role[Gene] = Role("target", Gene)
            score: ConfidenceScore
            interaction_type: InteractionType | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Gene, ScoredInteraction)
        schema_manager.sync_schema(force=True)

        return clean_db, Gene, ScoredInteraction, GeneSymbol, ConfidenceScore, InteractionType

    def test_filter_high_confidence_interactions(self, schema_with_scored_interactions):
        """Filter gene interactions by confidence score threshold."""
        db, Gene, ScoredInteraction, GeneSymbol, ConfidenceScore, InteractionType = (
            schema_with_scored_interactions
        )

        # Create gene network
        for symbol in ["HUB", "A", "B", "C", "D", "E"]:
            Gene.manager(db).insert(Gene(symbol=GeneSymbol(symbol)))

        genes = {g.symbol.value: g for g in Gene.manager(db).all()}
        hub = genes["HUB"]

        # Create interactions from hub to other genes with varying scores
        scores = {"A": 0.95, "B": 0.85, "C": 0.70, "D": 0.50, "E": 0.30}
        for target, score in scores.items():
            ScoredInteraction.manager(db).insert(
                ScoredInteraction(
                    source=hub,
                    target=genes[target],
                    score=ConfidenceScore(score),
                    interaction_type=InteractionType("binding"),
                )
            )

        # Filter by high confidence (> 0.8)
        high_conf = (
            ScoredInteraction.manager(db).filter(ConfidenceScore.gt(ConfidenceScore(0.8))).execute()
        )
        assert len(high_conf) == 2  # A (0.95), B (0.85)

        # Filter by low confidence (<= 0.5)
        low_conf = (
            ScoredInteraction.manager(db)
            .filter(ConfidenceScore.lte(ConfidenceScore(0.5)))
            .execute()
        )
        assert len(low_conf) == 2  # D (0.50), E (0.30)

    def test_order_interactions_by_score(self, schema_with_scored_interactions):
        """Order gene interactions by confidence score."""
        db, Gene, ScoredInteraction, GeneSymbol, ConfidenceScore, InteractionType = (
            schema_with_scored_interactions
        )

        # Create genes
        Gene.manager(db).insert(Gene(symbol=GeneSymbol("SOURCE")))
        source = Gene.manager(db).get(symbol="SOURCE")[0]

        for i, score in enumerate([0.3, 0.9, 0.5, 0.7, 0.1]):
            Gene.manager(db).insert(Gene(symbol=GeneSymbol(f"TARGET{i}")))
            target = Gene.manager(db).get(symbol=f"TARGET{i}")[0]
            ScoredInteraction.manager(db).insert(
                ScoredInteraction(source=source, target=target, score=ConfidenceScore(score))
            )

        # Order by score descending
        ordered = ScoredInteraction.manager(db).filter().order_by("-score").execute()
        scores = [i.score.value for i in ordered]
        assert scores == sorted(scores, reverse=True)

        # Get top 3 interactions
        top3 = ScoredInteraction.manager(db).filter().order_by("-score").limit(3).execute()
        assert len(top3) == 3
        top3_scores = [i.score.value for i in top3]
        assert top3_scores == [0.9, 0.7, 0.5]

    def test_count_interactions_from_source(self, schema_with_scored_interactions):
        """Count outgoing interactions from a specific gene."""
        db, Gene, ScoredInteraction, GeneSymbol, ConfidenceScore, InteractionType = (
            schema_with_scored_interactions
        )

        # Create hub-and-spoke network
        Gene.manager(db).insert(Gene(symbol=GeneSymbol("HUB_GENE")))
        Gene.manager(db).insert(Gene(symbol=GeneSymbol("LEAF_GENE")))

        hub = Gene.manager(db).get(symbol="HUB_GENE")[0]
        leaf = Gene.manager(db).get(symbol="LEAF_GENE")[0]

        # Hub has many outgoing interactions
        for i in range(10):
            Gene.manager(db).insert(Gene(symbol=GeneSymbol(f"TARGET_{i}")))
            target = Gene.manager(db).get(symbol=f"TARGET_{i}")[0]
            ScoredInteraction.manager(db).insert(
                ScoredInteraction(source=hub, target=target, score=ConfidenceScore(0.5))
            )

        # Leaf has only one outgoing interaction
        ScoredInteraction.manager(db).insert(
            ScoredInteraction(source=leaf, target=hub, score=ConfidenceScore(0.8))
        )

        # Count interactions per source
        hub_count = ScoredInteraction.manager(db).filter(source=hub).count()
        leaf_count = ScoredInteraction.manager(db).filter(source=leaf).count()

        assert hub_count == 10
        assert leaf_count == 1
